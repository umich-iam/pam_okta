# SPDX-License-Identifier: MIT
# SPDX-FileCopyrightText: © 2025 Regents of The University of Michigan
#
# This file is part of pam_okta and is distributed under the terms of
# the MIT license.

%global selinuxtype targeted
%global moduletype contrib
%global modulename pam_okta

%if 0%{?rhel} < 10
%define _debugsource_template %{nil}
%endif

%global rawversion 0.5.0-alpha.3

Name: pam_okta
Version: %(v=%{rawversion}; echo ${v/-/\~})
Release: 1%{?dist}
Summary: PAM module for Okta
# Not supported on EL8
#SourceLicense: MIT
# Generated summary with `env RUSTC_BOOTSTRAP=1 cargo tree -Z avoid-dev-deps --edges no-build,no-dev,no-proc-macro --no-dedupe --prefix none --format "# {l}" | sed -e "s: / :/:g" -e "s:/: OR :g" | sort -u`
License: MIT AND Apache-2.0 AND ISC AND BSD-3-Clause AND CDLA-Permissive-2.0 AND GPL-3.0
URL: https://github.com/umich-iam/pam_okta
Source: %{name}-%{rawversion}.crate
BuildRequires: pam-devel
BuildRequires: rust-toolset
BuildRequires: selinux-policy-devel
Requires: (%{name}-selinux if selinux-policy-%{selinuxtype})

%description
PAM module for Okta.

%package selinux
Summary: SELinux rules for %{name}
BuildArch: noarch
Requires: selinux-policy-%{selinuxtype}
Requires(post): selinux-policy-%{selinuxtype}
Requires: pam_okta%{?_isa} = %{version}-%{release}
%{?selinux_requires}

%description selinux
%{summary}.

%prep
%autosetup -n %{name}-%{rawversion} -p1

%build
make TARGET_PROFILE=rpm

%install
%make_install \
    TARGET_PROFILE=rpm \
    prefix=%{_prefix} \
    libdir=%{_libdir} \
    datarootdir=%{_datarootdir}

%files
%license LICENSE LICENSES.dependencies
%doc README.md CHANGELOG.md
%{_mandir}/man8/pam_okta.8*
%defattr(-,root,root)
%{_libdir}/security/pam_okta.so

%files selinux
%{_datadir}/selinux/packages/%{modulename}.pp.bz2

%pre selinux
%selinux_relabel_pre -s %{selinuxtype}

%post selinux
%selinux_modules_install %{_datadir}/selinux/packages/%{modulename}.pp.bz2

%postun selinux
if [ $1 -eq 0 ]; then
    %selinux_modules_uninstall %{_datadir}/selinux/packages/%{modulename}.pp.bz2
fi

%posttrans selinux
%selinux_relabel_post -s %{selinuxtype}

%changelog
* %(date "+%a %b %d %Y") (Automated RPM build) - %{version}-%{release}
- See git log for actual changes.
