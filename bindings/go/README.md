# Go Binding

`github.com/ieee0824/omoikane/bindings/go` は、Omoikane の C FFI を包む `cgo` ラッパーです。

## 前提

最初に Rust 側の共有ライブラリを生成します。

```bash
cargo build
```

macOS では `target/debug/libomoikane.dylib`、Linux では `target/debug/libomoikane.so` を参照します。

## インストール

```bash
go get github.com/ieee0824/omoikane/bindings/go
```

## 使い方

```go
package main

import (
	"fmt"
	omoikane "github.com/ieee0824/omoikane/bindings/go"
)

func main() {
	browser, err := omoikane.NewBrowser()
	if err != nil {
		panic(err)
	}
	defer browser.Close()

	if err := browser.Navigate(`data:text/html,<html><body><main id="app">hello</main></body></html>`); err != nil {
		panic(err)
	}

	result, err := browser.Evaluate(`document.getElementById("app").nodeName`)
	if err != nil {
		panic(err)
	}

	fmt.Println(string(result))
}
```

## テスト

```bash
cargo build
cd bindings/go
go test ./...
```
