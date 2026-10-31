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

prepare() {
	cargo fetch --frozen
}

build() {
	local dir
	dir="$(find "$srcdir" -maxdepth 1 -type d -name "*-${pkgver}" | head -n1)"
	cd "$dir"
	cargo build --release --frozen
}

check() {
	local dir
	dir="$(find "$srcdir" -maxdepth 1 -type d -name "*-${pkgver}" | head -n1)"
	cd "$dir"
	cargo test --release --frozen
}

package() {
	install -Dm755 "$srcdir/vorgan-$pkgver/target/release/vorgan" "$pkgdir/usr/bin/vorgan"
	install -Dm644 "$srcdir/vorgan-$pkgver/README.md" "$pkgdir/usr/share/licenses/$pkgname/README"
	install -Dm644 "$srcdir/vorgan-$pkgver/LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
