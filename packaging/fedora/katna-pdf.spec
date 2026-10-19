# SPDX-License-Identifier: GPL-3.0-or-later
# Fedora package for Katna PDF, made from the plain Linux download
# (packaging/linux/make-tarball.sh) rather than built here:
#
#   rpmbuild -bb --define "_sourcedir DIR" --define "pkgversion VERSION" \
#     packaging/fedora/katna-pdf.spec
#
# with katna-pdf-linux-x86_64.tar.gz in DIR.

Name:           katna-pdf
Version:        %{pkgversion}
Release:        1%{?dist}
Summary:        Fast PDF reader, editor and signer
License:        GPL-3.0-or-later
URL:            https://github.com/QuakeString/katna-pdf
Source0:        katna-pdf-linux-x86_64.tar.gz
ExclusiveArch:  x86_64
# Loaded at run time rather than linked, so not found by rpm itself.
Requires:       libwayland-client
Requires:       vulkan-loader
Requires:       libglvnd-egl
Recommends:     mesa-vulkan-drivers
Recommends:     xdg-desktop-portal
Suggests:       speech-dispatcher
Suggests:       espeak-ng
Suggests:       opensc
Suggests:       pcsc-lite

# Built and stripped by cargo, without build ID notes.
%global debug_package %{nil}
%global __strip /bin/true
%undefine _missing_build_ids_terminate_build

%description
Katna PDF reads, edits, fills, signs and organizes PDF files, and opens
large technical drawings quickly. It signs with certificate files and
USB tokens (PAdES), and keeps a library of your PDFs. This package holds
the app and katna-pdf-cli, its command-line tool.

%prep
%setup -q -n katna-pdf-linux-x86_64

%install
mkdir -p %{buildroot}%{_prefix}
cp -a bin share %{buildroot}%{_prefix}/
rm -r %{buildroot}%{_datadir}/licenses

%files
%license share/licenses/katna-pdf/LICENSE
%{_bindir}/katna-pdf
%{_bindir}/katna-pdf-cli
%{_datadir}/applications/*.desktop
%{_datadir}/metainfo/*.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/*.svg
