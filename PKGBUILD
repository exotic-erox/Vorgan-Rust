# Maintainer: exotic-erox <exotic-erox@users.noreply.github.com>
pkgname=vorgan
pkgver=0.2.1
pkgrel=1
pkgdesc="Terminal file manager with a particle-graph navigator (Rust)"
arch=('x86_64' 'aarch64')
url="https://github.com/exotic-erox/Vorgan-Rust"
license=('MIT')
makedepends=('cargo' 'git')
optdepends=('libarchive: archive browsing, extract and compress'
            'wl-clipboard: copy path to clipboard'
            'xdg-utils: open files and terminals')
source=("vorgan-$pkgver.tar.gz::https://github.com/exotic-erox/Vorgan-Rust/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')
options=('!strip')

_dir="Vorgan-Rust-$pkgver"

prepare() {
	cd "$srcdir/$_dir"
	cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
	cd "$srcdir/$_dir"
	cargo build --release --frozen
}

check() {
	cd "$srcdir/$_dir"
	cargo test --release --frozen
}

package() {
	cd "$srcdir/$_dir"
	install -Dm755 target/release/vorgan "$pkgdir/usr/bin/vorgan"
	install -Dm644 README.md "$pkgdir/usr/share/licenses/$pkgname/README"
	install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
