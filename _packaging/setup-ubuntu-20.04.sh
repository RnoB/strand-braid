#!/bin/bash -x
set -o errexit

apt-get update
apt-get install -y cpio libudev-dev libapriltag-dev libssl-dev zlib1g-dev pkg-config

ORIG_DIR=`pwd`
echo $ORIG_DIR

# Install IPP
mkdir -p /tmp/download-ipp
cd /tmp/download-ipp
curl -O --silent https://internal-static.strawlab.org/software/ipp/l_ipp_2019.3.199.tgz
curl -O --silent https://internal-static.strawlab.org/software/ipp/install-ipp-2019.sh
chmod a+x install-ipp-2019.sh
/tmp/download-ipp/install-ipp-2019.sh
cd /
rm -rf /tmp/download-ipp

# Install nightly Rust
cd /tmp
curl -O --silent https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init && chmod a+x rustup-init && ./rustup-init -y --default-toolchain nightly

# Note: this is not a good general-purpose way to install wasm-pack, because it does not install wasm-bindgen.
# Instead, use the installer at https://rustwasm.github.io/wasm-pack/installer/.
mkdir -p $CARGO_HOME/bin && curl --silent https://internal-static.strawlab.org/software/wasm-pack/wasm-pack-0.8.1-amd64.exe > $CARGO_HOME/bin/wasm-pack
chmod a+x $CARGO_HOME/bin/wasm-pack
export PATH="$PATH:$CARGO_HOME/bin"
wasm-pack --version

# TODO: include firmware bundled
rustc --version
curl --silent https://internal-static.strawlab.org/software/libvpx/libvpx-opt-static_1.8.0-0ads1_amd64.deb > /tmp/libvpx-opt-static_1.8.0-0ads1_amd64.deb
dpkg -i /tmp/libvpx-opt-static_1.8.0-0ads1_amd64.deb

# Download pylon and install 6
curl --silent https://internal-static.strawlab.org/software/pylon/pylon_6.1.1.19861-deb0_amd64.deb > /tmp/pylon_6.1.1.19861-deb0_amd64.deb
echo "e738adb36f117ff2e5c428670025f9dfcdfbcbc9b22e2e2924a10736f876f2ed /tmp/pylon_6.1.1.19861-deb0_amd64.deb" | sha256sum -c
dpkg -i /tmp/pylon_6.1.1.19861-deb0_amd64.deb

curl --silent https://internal-static.strawlab.org/software/opencv/opencv-3.2-static.tar.gz > /tmp/opencv-3.2-static.tar.gz
tar xzf /tmp/opencv-3.2-static.tar.gz -C /

mkdir -p $CARGO_HOME/bin && curl --silent https://internal-static.strawlab.org/software/cargo-web/cargo-web-0.6.25-amd64.exe > $CARGO_HOME/bin/cargo-web

cd $ORIG_DIR
