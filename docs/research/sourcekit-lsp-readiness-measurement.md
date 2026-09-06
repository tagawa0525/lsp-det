# sourcekit-lsp の readiness の実測（M18。保留）

ADR 0019 決定 F の M18。コーパス（[readiness-vocabulary-corpus.md](readiness-vocabulary-corpus.md)）は sourcekit-lsp を「信号はあるが設定で無効（opt-in）」型に置き、「観測者が `backgroundIndexing` を注入して `indexing` → `ready` を取れるか」を疑問にしていた。**この疑問は nixpkgs の版では測れない。** `backgroundIndexing` と `IndexProgressManager`（title "Indexing" の `$/progress`）は Swift 6.0 以降のもので、nixpkgs（固定した rev も nixos-unstable も）の sourcekit-lsp は 5.10.1。5.10.1 は索引をビルド（`swift build --enable-index-store`）からしか作らず、しかも nixpkgs の Swift 5.10.1 には IndexStoreDB が索引を読むのに要る `libIndexStore.so` が入っていないので、ビルドしても `references` と `workspace/symbol` は空のまま。実物（6.x）が取れるまで保留し、5.10.1 で分かったことだけを記す。

## 方法

- nixpkgs の sourcekit-lsp 5.10.1、swift 5.10.1、swiftpm 5.10.1（`flake.nix` には足していない）。2026-09-06
- 被験体: SwiftPM の library（`Package.swift`、`Sources/Fixture/A.swift` の `public func target()`、`B.swift` の `x()` が呼ぶ）
- nixpkgs の swiftpm は `swift build` の manifest のコンパイルが `libdispatch.so` を見つけられず失敗する。`swiftPackages.Dispatch` と `Foundation` の `lib` を `LD_LIBRARY_PATH` に足すと通る。sourcekit-lsp も同じ理由で SwiftPM のワークスペースを作れず（"failed to create SwiftPMWorkspace"、"no such module 'PackageDescription'"）、`bin` を swift-wrapper に、`lib/swift/pm` を swiftpm に向けた合成ツールチェーンを `SOURCEKIT_TOOLCHAIN_PATH` で渡すと読み込める
- 走行: (1) ビルドなし、(2) `swift build`（既定では index store を作らない）、(3) `swift build --enable-index-store` の後（`.build/<triple>/debug/index/store` ができる）、`workspace/_pollIndex` も送る

## 結果

### 語彙（5.10.1）

| 信号                                                                                             | 内容                                                                                                                                                                                             |
| ------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `$/progress`（token "SourceKitLSP.SoruceKitServer.reloadPackage"。綴りはサーバーのソースのまま） | title "Reloading Package"。`initialized` の直後に create と end だけが届く（begin は create の応答を待つ間に終わる）。SwiftPM の manifest の読み込みで、索引ではない                             |
| `workspace/_pollIndex`（要求）                                                                   | クライアントが送ると、index store の未処理の unit を処理し終わるまで待って空の応答を返す（`pollForUnitChangesAndWait`）。0.1〜0.3 秒。**索引の同期をクライアントが要求する語彙**で、通知ではない |

`serverInfo` は null。`backgroundIndexing` の初期化オプションは 5.10.1 にはなく、渡しても何も起きない。索引は `swift build --enable-index-store` が書く index store を IndexStoreDB で読む設計で、サーバー自身は作らない。

### 走行 1〜3

| 条件                                                                   | `references` / `workspace/symbol`          |
| ---------------------------------------------------------------------- | ------------------------------------------ |
| ビルドなし                                                             | 空 / 0 件（20 秒）                         |
| `swift build`（index store なし）                                      | 空 / 0 件                                  |
| `--enable-index-store` でビルド、合成ツールチェーンで package を読めた | **空 / 0 件のまま**。stderr にも何も出ない |

最後の条件で空なのは、nixpkgs の Swift 5.10.1 に `libIndexStore.so`（`swift-unwrapped` の `lib` に存在しない）がなく、IndexStoreDB が index store を開けないためと考えられる。sourcekit-lsp は開けなかったことを `window/logMessage` にも `window/showMessage` にも出さず、以後すべての横断要求に空の成功応答を返す。**壊れたサーバーの成功風応答**で、health の信号はない。

## 結論

- `backgroundIndexing` の注入（決定 G）を測るには 6.x が要る。nixpkgs にはなく、swift.org の tarball は NixOS で FHS 環境が要る。nixpkgs が 6.x に上がったら測る（M11 Kotlin と同じ扱い）
- 5.10.1 の語彙で新しいのは `workspace/_pollIndex`。「索引の同期を要求する」という、通知でなく要求で readiness を扱う形。仕様 8 章の観測者が使う候補にはなるが（保留を解く前に送る）、観測者が要求を注入することは決定 G の範囲外（信号の有効化ではなく要求の代行）で、採らない
- 写像は書かず、`flake.nix` にも足さない。`serverInfo` がないので lsp-det は両軸 `unknown` のまま
