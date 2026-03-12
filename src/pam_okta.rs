// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: © 2025 Regents of The University of Michigan
//
// This file is part of pam_okta and is distributed under the terms of
// the MIT license.

use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;

#[rustfmt::skip]
use pamsm::{
    LogLvl,
    Pam,
    PamError,
    PamFlags,
    PamLibExt,
    PamMsgStyle,
    PamServiceModule,
    pam_module,
};

struct PamOkta;

#[derive(serde::Deserialize)]
struct OktaConfig {
    host: String,
    client_id: String,
    client_secret: String,
    #[serde(default)]
    http_proxy: String,
    #[serde(default)]
    debug: bool,
}

struct OktaHandle<'a> {
    pamh: &'a Pam,
    conf: OktaConfig,
    agent: ureq::Agent,
    mfa_token: Option<String>,
}

fn ureq_config_base() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    ureq::Agent::config_builder()
        .user_agent(format!("pam_okta/{}", env!("CARGO_PKG_VERSION")))
        .http_status_as_error(false)
}

impl OktaHandle<'_> {
    fn log_error(&self, msg: &str) {
        let _ = self.pamh.syslog(LogLvl::NOTICE, msg);
    }

    fn log_info(&self, msg: &str) {
        let _ = self.pamh.syslog(LogLvl::INFO, msg);
    }

    fn log_debug(&self, msg: &str) {
        if self.conf.debug {
            self.log_info(msg);
        }
    }

    fn send_error(&self, msg: &str) {
        self.log_error(msg);
        let _ = self.pamh.conv(Some(msg), PamMsgStyle::ERROR_MSG);
    }

    fn send_info(&self, msg: &str) {
        self.log_info(msg);
        let _ = self.pamh.conv(Some(msg), PamMsgStyle::TEXT_INFO);
    }

    fn send_important_info(&self, msg: &str) -> Result<(), PamError> {
        // Unfortunately, OpenSSH authentication doesn't display non-prompt
        // messages as it goes so we have to display a useless prompt.
        if self
            .pamh
            .get_service()
            .unwrap_or_default()
            .unwrap_or_default()
            .to_str()
            .unwrap_or_default()
            == "sshd"
        {
            self.pamh.conv(
                Some(&format!("{msg}\nPress enter to continue: ")),
                PamMsgStyle::PROMPT_ECHO_OFF,
            )?;
        } else {
            self.pamh.conv(Some(msg), PamMsgStyle::TEXT_INFO)?;
        }
        Ok(())
    }

    fn configure_agent(&mut self) {
        if !self.conf.http_proxy.is_empty() {
            let proxy = ureq::Proxy::new(&self.conf.http_proxy);
            if let Ok(proxy_val) = proxy {
                self.agent = ureq_config_base().proxy(Some(proxy_val)).build().into();
            } else {
                self.log_info(&format!(
                    "Ignoring invalid HTTP proxy: {}",
                    self.conf.http_proxy
                ));
            }
        }
    }

    fn post(
        &self,
        endpoint: &str,
        form_data: &[(&str, &str)],
        log_http_errors: bool,
    ) -> Result<(bool, Option<serde_json::Value>), ureq::Error> {
        let url = format!("https://{}/oauth2/v1/{endpoint}", self.conf.host);

        let mut resp = self.agent.post(&url).send_form(form_data.to_owned())?;
        if log_http_errors && !resp.status().is_success() {
            self.log_error(&format!("HTTP {}", resp.status()));
        } else {
            self.log_debug(&format!("HTTP {}", resp.status()));
        }

        match resp.body_mut().read_json::<serde_json::Value>() {
            Ok(res) => {
                self.log_debug(&res.to_string());
                Ok((resp.status().is_success(), Some(res)))
            }
            Err(e) => {
                self.log_info(&e.to_string());
                Ok((resp.status().is_success(), None))
            }
        }
    }

    fn poll_for_token(
        &self,
        form_data: &[(&str, &str)],
        interval: std::time::Duration,
        timeout: u64,
    ) -> Result<serde_json::Value, PamError> {
        let now = std::time::Instant::now();

        while now.elapsed().as_secs() <= timeout {
            std::thread::sleep(interval);

            let resp_json = match self.post("token", form_data, false) {
                Ok((false, Some(res))) => res,
                Ok((true, Some(res))) => {
                    return Ok(res);
                }
                // HTTP success, but the body was broken
                Ok((true, None)) => {
                    return Err(PamError::AUTHINFO_UNAVAIL);
                }
                Ok((false, None)) | Err(_) => {
                    continue;
                }
            };

            if resp_json["error"].as_str().unwrap_or_default() != "authorization_pending" {
                self.send_error(&format!(
                    "Polling failed: {}",
                    resp_json["error_description"].as_str().unwrap_or_default()
                ));
                return Err(PamError::AUTH_ERR);
            }
        }

        Err(PamError::AUTH_ERR)
    }

    fn factor_otp(&self, username: &str, otp: &str) -> PamError {
        self.log_info(&format!("Attempting OTP authentication for {username}"));

        let mut form_data = vec![
            ("client_id", self.conf.client_id.as_str()),
            ("client_secret", self.conf.client_secret.as_str()),
            ("scope", "openid"),
            ("otp", otp),
        ];

        if let Some(tok) = &self.mfa_token {
            form_data.push(("grant_type", "http://auth0.com/oauth/grant-type/mfa-otp"));
            form_data.push(("mfa_token", tok.as_str()));
        } else {
            form_data.push(("grant_type", "urn:okta:params:oauth:grant-type:otp"));
            form_data.push(("login_hint", username));
        }

        match self.post("token", &form_data, true) {
            Ok((true, _)) => {
                self.send_info("OTP authentication succeeded");
                PamError::SUCCESS
            }
            Ok((false, _)) => {
                self.send_error("OTP authentication failed");
                PamError::AUTH_ERR
            }
            Err(e) => {
                self.log_error(&e.to_string());
                self.send_error("OTP authentication failed");
                PamError::AUTHINFO_UNAVAIL
            }
        }
    }

    fn factor_password(&mut self, username: &str, password: &str) -> Option<PamError> {
        self.log_info(&format!(
            "Attempting password authentication for {username}"
        ));
        let form_data = [
            ("client_id", self.conf.client_id.as_str()),
            ("client_secret", self.conf.client_secret.as_str()),
            ("grant_type", "password"),
            ("scope", "openid"),
            ("username", username),
            ("password", password),
        ];

        let resp_json = match self.post("token", &form_data, true) {
            Ok((true, _)) => {
                self.send_info("Password authentication succeeded");
                return Some(PamError::SUCCESS);
            }
            Ok((false, Some(res))) => res,
            Ok((false, None)) => {
                self.send_error("Password authentication failed");
                return Some(PamError::AUTHINFO_UNAVAIL);
            }
            Err(e) => {
                self.log_error(&e.to_string());
                self.send_error("Password authentication failed");
                return Some(PamError::AUTHINFO_UNAVAIL);
            }
        };

        let err = resp_json["error"].as_str().unwrap_or_default();
        if err == "mfa_required" {
            self.mfa_token = Some(String::from(
                resp_json["mfa_token"].as_str().unwrap_or_default(),
            ));
            self.send_info(resp_json["error_description"].as_str().unwrap_or_default());
            return None;
        }
        self.log_info(&format!("Password authentication failed: {err}"));

        Some(PamError::AUTH_ERR)
    }

    fn factor_push(&self, username: &str) -> PamError {
        self.log_info(&format!("Attempting push authentication for {username}"));

        let mut push_url = String::new();
        let mut form_data = vec![
            ("client_id", self.conf.client_id.as_str()),
            ("client_secret", self.conf.client_secret.as_str()),
            ("channel_hint", "push"),
        ];

        if let Some(tok) = &self.mfa_token {
            push_url.push_str("challenge");
            form_data.push((
                "challenge_types_supported",
                "http://auth0.com/oauth/grant-type/mfa-oob",
            ));
            form_data.push(("mfa_token", tok.as_str()));
        } else {
            push_url.push_str("oob-authenticate");
            form_data.push(("login_hint", username));
        }

        let resp_json = match self.post(&push_url, &form_data, true) {
            Ok((true, Some(res))) => res,
            Ok(_) => {
                return PamError::AUTHINFO_UNAVAIL;
            }
            Err(e) => {
                self.log_error(&e.to_string());
                return PamError::AUTHINFO_UNAVAIL;
            }
        };

        self.send_info("Successfully initiated Okta push");

        let mut form_data = vec![
            ("client_id", self.conf.client_id.as_str()),
            ("client_secret", self.conf.client_secret.as_str()),
            ("scope", "openid"),
            (
                "oob_code",
                resp_json["oob_code"].as_str().unwrap_or_default(),
            ),
        ];

        if let Some(tok) = &self.mfa_token {
            form_data.push(("grant_type", "http://auth0.com/oauth/grant-type/mfa-oob"));
            form_data.push(("mfa_token", tok.as_str()));
        } else {
            form_data.push(("grant_type", "urn:okta:params:oauth:grant-type:oob"));
        }

        if let Some(num_challenge) = resp_json["binding_code"].as_str() {
            if let Err(e) =
                self.send_important_info(&format!("The correct answer is {num_challenge}"))
            {
                self.log_error("Number challenge prompt failed to display");
                return e;
            }
        }

        let timeout = resp_json["expires_in"].as_u64().unwrap_or(0);
        let interval = std::time::Duration::from_secs(resp_json["interval"].as_u64().unwrap_or(10));

        match self.poll_for_token(&form_data, interval, timeout) {
            Ok(_) => {
                self.send_info("Push acknowledged");
                PamError::SUCCESS
            }
            Err(e) => e,
        }
    }
}

impl PamServiceModule for PamOkta {
    fn authenticate(pamh: Pam, _: PamFlags, args: Vec<String>) -> PamError {
        let username = match pamh.get_user(None) {
            Ok(Some(user)) => user.to_str().unwrap_or_default(),
            Ok(None) => return PamError::SERVICE_ERR,
            Err(e) => return e,
        };

        let mut oh = OktaHandle {
            pamh: &pamh,
            conf: OktaConfig {
                host: String::new(),
                client_id: String::new(),
                client_secret: String::new(),
                http_proxy: String::new(),
                debug: false,
            },
            agent: ureq_config_base().build().into(),
            mfa_token: None,
        };

        oh.log_info(&format!("Authentication attempt for {username}"));

        let mut conf_path = String::from("/etc/security/pam_okta.toml");
        let mut autopush = false;
        let mut password_auth = false;
        let mut try_first_pass = false;
        let mut use_first_pass = false;

        for arg in args {
            match arg.split_once('=') {
                Some((k, v)) => match k {
                    "config_file" => {
                        conf_path = String::from(v);
                    }
                    _ => oh.log_info(&format!("Unknown PAM argument: {arg}")),
                },
                None => match arg.as_str() {
                    "autopush" => autopush = true,
                    "password_auth" => password_auth = true,
                    "try_first_pass" => try_first_pass = true,
                    "use_first_pass" => use_first_pass = true,
                    _ => oh.log_info(&format!("Unknown PAM argument: {arg}")),
                },
            }
        }

        // Avoid potential TOCTOU race condition by checking permissions on already opened file.
        // Note that the contents of file are read only _after_ permissions are verified.
        let mut conf_file = match File::open(conf_path) {
            Ok(file) => match file.metadata() {
                Ok(stat) if stat.permissions().mode() & 0o007 != 0o000 => {
                    oh.send_error(
                        "pam_okta configuration is unusable: unacceptable permissions",
                    );
                    return PamError::SERVICE_ERR;
                }
                Ok(_) => file,
                Err(e) => {
                    oh.send_error(&format!("pam_okta configuration is unusable: {e}"));
                    return PamError::SERVICE_ERR;
                }
            },
            Err(e) => {
                oh.send_error(&format!("pam_okta configuration is unusable: {e}"));
                return PamError::SERVICE_ERR;
            }
        };

        let mut conf_data = String::new();
        match conf_file.read_to_string(&mut conf_data) {
            Ok(_) => (),
            Err(e) => {
                oh.send_error(&format!("pam_okta configuration is unusable: {e}"));
                return PamError::SERVICE_ERR;
            }
        }

        if let Ok(conf) = toml::from_str(&conf_data) {
            oh.conf = conf;
        } else {
            oh.log_error("unexpected error parsing config file");
            return PamError::SERVICE_ERR;
        }
        oh.configure_agent();

        if password_auth {
            if try_first_pass || use_first_pass {
                let password = match pamh.get_cached_authtok() {
                    Ok(Some(pass)) => pass.to_str().unwrap_or_default(),
                    Ok(_) => "",
                    Err(e) => return e,
                };
                if let Some(res) = oh.factor_password(username, password) {
                    if use_first_pass || res == PamError::SUCCESS {
                        return res;
                    }
                } else {
                    password_auth = false;
                }
            }
            if password_auth {
                let password =
                    match pamh.conv(Some("Okta password: "), PamMsgStyle::PROMPT_ECHO_OFF) {
                        Ok(Some(pass)) => pass.to_str().unwrap_or_default(),
                        Ok(_) => "",
                        Err(e) => return e,
                    };
                if let Some(res) = oh.factor_password(username, password) {
                    return res;
                }
            }
        }

        if autopush {
            return oh.factor_push(username);
        }

        match pamh.conv(
            Some("Okta passcode (leave blank to initiate a push): "),
            PamMsgStyle::PROMPT_ECHO_ON,
        ) {
            Ok(Some(otp)) if !otp.to_str().unwrap_or_default().is_empty() => {
                oh.factor_otp(username, otp.to_str().unwrap_or_default())
            }
            Ok(_) => oh.factor_push(username),
            Err(e) => e,
        }
    }

    // The defaults return SERVICE_ERR, so we need to override most of them.
    fn open_session(_: Pam, _: PamFlags, _: Vec<String>) -> PamError {
        PamError::SUCCESS
    }
    fn close_session(_: Pam, _: PamFlags, _: Vec<String>) -> PamError {
        PamError::SUCCESS
    }
    fn setcred(_: Pam, _: PamFlags, _: Vec<String>) -> PamError {
        PamError::SUCCESS
    }
    fn acct_mgmt(_: Pam, _: PamFlags, _: Vec<String>) -> PamError {
        PamError::SUCCESS
    }
}

pam_module!(PamOkta);
