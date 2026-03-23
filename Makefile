# SPDX-License-Identifier: MIT
# SPDX-FileCopyrightText: © 2025 Regents of The University of Michigan
#
# This file is part of pam_okta and is distributed under the terms of
# the MIT license.

TARGET_PROFILE = release
INSTALL = install -p

datarootdir = ${prefix}/share
exec_prefix = ${prefix}
libdir = ${exec_prefix}/lib
mandir = ${datarootdir}/man
prefix = /usr/local

.MAKE: all

.PHONY: build deb doc install package rpm selinux

all: build selinux LICENSES.dependencies

LICENSES.dependencies: Cargo.lock
	env RUSTC_BOOTSTRAP=1 cargo tree -Z avoid-dev-deps --edges no-build,no-dev,no-proc-macro --no-dedupe --prefix none --format "{l}: {p}" | sed -e "s: ($(pwd)[^)]*)::g" -e "s: / :/:g" -e "s:/: OR :g" | sort -u > LICENSES.dependencies

doc/pam_okta.8.md: doc/pam_okta.8
	mandoc -T markdown doc/pam_okta.8 > doc/pam_okta.8.md

doc: doc/pam_okta.8.md

build:
	cargo build --locked --profile $(TARGET_PROFILE)

install:
	$(INSTALL) -m 0755 -d $(DESTDIR)$(libdir)/security
	$(INSTALL) -m 0755 target/$(TARGET_PROFILE)/libpam_okta.so $(DESTDIR)$(libdir)/security/pam_okta.so
	$(INSTALL) -m 0755 -d $(DESTDIR)$(mandir)/man8
	$(INSTALL) -m 0644 doc/pam_okta.8 $(DESTDIR)$(mandir)/man8
	$(INSTALL) -m 0755 -d $(DESTDIR)$(datarootdir)/selinux/packages
	$(INSTALL) -m 0644 target/pam/pam_okta.pp $(DESTDIR)$(datarootdir)/selinux/packages
	bzip2 -9 $(DESTDIR)$(datarootdir)/selinux/packages/pam_okta.pp

package:
	cargo package --no-verify

target/pam/pam_okta.mod: src/pam_okta.te
	mkdir -p target/pam
	checkmodule -M -m -o target/pam/pam_okta.mod src/pam_okta.te

target/pam/pam_okta.pp: target/pam/pam_okta.mod
	semodule_package -o target/pam/pam_okta.pp -m target/pam/pam_okta.mod

selinux: target/pam/pam_okta.pp

rpm: package
	rpmbuild -ta target/package/pam_okta-0.5.0.crate

deb: build
	mkdir -p deb
	nfpm package -p deb -f packaging/deb/nfpm.yaml -t deb
