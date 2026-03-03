<!--
SPDX-License-Identifier: MIT
SPDX-FileCopyrightText: © 2025 Regents of The University of Michigan

This file is part of pam_okta_auth and is distributed under the terms of
the MIT license.
-->
# pam\_okta\_auth

[![build status](https://github.com/umich-iam/okta-pam-auth/actions/workflows/build.yml/badge.svg)](https://github.com/umich-iam/okta-pam-auth/actions/workflows/build.yml) [![dependencies status](https://github.com/umich-iam/okta-pam-auth/actions/workflows/dependencies.yml/badge.svg)](https://github.com/umich-iam/okta-pam-auth/actions/workflows/dependencies.yml)

Okta authentication for Unix systems.

![animated demo](doc/pam_okta_auth.gif)

`pam_okta_auth` is a Pluggable Authentication Modules (PAM)
module designed to provide secondary authentication similar to
[`duo_unix`](https://github.com/duosecurity/duo_unix) using Okta.
It also has experimental support for password-based primary
authentication.

## Dependencies

`pam_okta_auth` is developed and used mainly on Linux systems using
[Linux-PAM](https://github.com/linux-pam/linux-pam), but should be
compatible with other Unix-like systems and PAM implementations.

In order to build `pam_okta_auth` you will need the following:

* A [Rust](https://www.rust-lang.org/) compiler with support for the 2021 edition of Rust.
* [Cargo](https://doc.rust-lang.org/cargo/)
* PAM

You can install these dependencies on most RPM-based systems by running
`dnf install pam-devel rust-toolset`,  and on Debian by running
`apt install libpam-dev rust-all`.

## Installation

Prebuilt RPM and deb packages are published via [GitHub
Releases](https://github.com/umich-iam/okta-pam-auth/releases/latest).

Example installation process for RHEL:
```
dnf install https://github.com/flowerysong/pam_okta_auth/releases/download/v0.1.3/pam_okta_auth-0.1.3-1.el9.x86_64.rpm https://github.com/flowerysong/pam_okta_auth/releases/download/v0.1.3/pam_okta_auth-selinux-0.1.3-1.el9.noarch.rpm
```

Example installation process for Ubuntu:
```
wget https://github.com/flowerysong/pam_okta_auth/releases/download/v0.1.3/pam_okta_auth_0.1.3_amd64.deb
dpkg -i pam_okta_auth_0.1.3_amd64.deb
```

### Manual Installation

In a git checkout (or a source tree obtained by other methods):
```
cargo build --locked --profile release
sudo install -m 0755 target/release/libpam_okta_auth.so /usr/lib/security/pam_okta_auth.so
```

`/usr/lib/security` is probably not the correct installation path for
your system. You should figure out where PAM expects modules to live
and adjust your process accordingly.

## Deployment

The configuration file, by default located at
`/etc/security/pam_okta_auth.toml`, uses the [TOML](https://toml.io/)
format. This file contains secrets so it must not be world readable.

Supported configuration file options and PAM options are documented
in the [man page](doc/pam_okta_auth.8.md).

Okta client credentials are required. These should be for a native
application with at least the `OTP` and `OOB` direct auth grants.

![Okta application settings](doc/okta_app_grants.png)

The application must also be assigned an authentication policy that
permits authentication with a single factor.

![Okta authentication policy](doc/okta_policy.png)

### Example Configuration File

```toml
host = "example.oktapreview.com"
client_id = "0deadgoffdeADGOffick"
client_secret = "6zFfFfffzfZFz6zFZFzzFZFZFfZf6Fz6F6ZfZ6f-FFFzZZ6FZ_zZFzFZ6fFzfFFz"
```

### Example PAM Configurations

```
auth    required    pam_okta_auth.so
```

`pam_duo` has a flag to "fail safe" and return `success` when there
is a configuration issue or the Duo service is unavailable. There is
no corresponding `pam_okta_auth` configuration—you can instead use
`Linux-PAM` controls to ignore the `service_err` and/or `authinfo_unavail`
returns from the module:

```
auth    [success=ok ignore=ignore authinfo_unavail=ignore service_err=ignore default=bad]   pam_okta_auth.so
```

`pam_duo` allows you to use a custom pattern language in its
configuration file to specify which groups should be required to
use Duo authentication. There is no equivalent functionality in
`pam_okta_auth`, but you can achieve similar configurations using
features available in the `Linux-PAM` stack.

```
# Only require Okta authentication for staff who aren't in the bypass group
auth    [default=1 ignore=ignore success=ignore]    pam_succeed_if.so quiet user ingroup staff user notingroup bypass
auth    required                                    pam_okta_auth.so
```

## Deployment As Primary Authentication

The password authentication flow requires client credentials for an
app with at least the `Resource Owner Password` grant; if the the
authentication policy assigned to the app requires MFA it will also
need the `MFA OTP` and `MFA OOB` grants.

This flow currently assumes that OTP (passcode) and push
authentication are always acceptable second factors when MFA is
required. It's still possible to apply a policy where only one of them
is allowed, but the end user experience is not ideal.

## OpenSSH and PAM

### Login Timeouts

While Okta out-of-band authentication normally gives users five
minutes to respond, OpenSSH in its default configuration will
only keep a connection open for 120 seconds without a successful
authentication. Exceeding this limit will result in the connection
silently dropping with no useful feedback to the user and no log
output on the server.

The internet likes to recommend reducing the `LoginGraceTime`, but
when deploying this software you might want to consider increasing
it instead. Setting it to 330 seconds will give people ample time
to complete the more complex authentication steps and minimize the
likelihood of an acknowledged push not resulting in successful
authentication.

### Poor Handling of Informational Messages

There are two known issues with the integration between OpenSSH and
PAM.

Firstly, non-prompt messages are buffered and shown after authentication
completes instead of being displayed to the user immediately.

There have been several attempts to fix this behaviour since it was
reported in [2018](https://bugzilla.mindrot.org/show_bug.cgi?id=2876),
but none of them have been accepted yet.

Consequently if we have a message—like a number challenge—that
absolutely must be displayed to the user, `pam_okta_auth` detects that
this authentication is being done for `sshd` and sends the message as
a prompt that the user must then acknowledge by hitting `enter`.

Secondly, this buffer accumulates non-prompt messages regardless
of whether PAM indicates they are errors or information, then logs
them at the `INFO` level. For successful authentications the client
also then treats them as a login message returned by the server and
displays them.

At the default `LogLevel INFO` this results in two copies on
successful login and one on failure:

```console
Okta passcode (leave blank to initiate a push):
Successfully initiated Okta push
Polling failed: User rejected out-of-band authentication prompt.
Okta passcode (leave blank to initiate a push):
Successfully initiated Okta push
Push acknowledged
Successfully initiated Okta push
Push acknowledged
```

Switching the client to `LogLevel ERROR` results in one copy of each
message on success… and zero on failure:

```console
Okta passcode (leave blank to initiate a push):
Okta passcode (leave blank to initiate a push):
Successfully initiated Okta push
Push acknowledged
```
