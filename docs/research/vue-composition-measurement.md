# Vue の相方サーバーとの合成の実測（M13）

ADR 0019 決定 B-5 の測定。Vue 3 の言語サーバー（`@vue/language-server` 3.3.10、Hybrid Mode）は `.vue` の横断（`references` 等）を自分では答えず、`@vue/typescript-plugin` を載せた tsserver（typescript-language-server 経由）に委ねる。クライアントはサーバーを 2 つ起動し、接続ごとに lsp-det を挟む。「接続ごとの保留で結果が完全になるか」を確かめた。**成り立つ**。横断の答えは typescript-language-server の接続から来て、その接続の lsp-det（M6 の写像）が "Initializing JS/TS language features…" の end まで保留するので、最初の答えから `.vue` の参照を含めて完全。vue-language-server の接続は横断を答えず、readiness の語彙も持たないので両軸 `unknown`（写像なし）でよい。2 つの状態を AND するのはクライアントの責務で、lsp-det は隣の接続を知らなくてよい。

## 方法

- typescript-language-server 5.3.0（nixpkgs。tsserver は被験体の `node_modules/typescript` 5.9.3）、`@vue/language-server` 3.3.10（nixpkgs の `vue-language-server`）、`@vue/typescript-plugin` 3.3.10、vue 3.5.13（被験体の `node_modules`。pnpm）。2026-09-06
- 被験体: `tsconfig.json`（`compilerOptions.plugins: [{"name": "@vue/typescript-plugin"}]`、`include` に `src/**/*.vue`）、`src/a.ts`（`export function target()`）、`src/c.ts`（`target()` を呼ぶ）、`src/B.vue`（`<script setup lang="ts">` で `target()` を呼ぶ）、`src/shims-vue.d.ts`
- 走行 A: typescript-language-server を直接、`initializationOptions.plugins` に `{"name": "@vue/typescript-plugin", "location": "<node_modules>/@vue/typescript-plugin", "languages": ["vue"]}`（Serena と同じ）。`didOpen src/a.ts`、`target` の `references` を 0.1 秒間隔
- 走行 A': 同じ構成を lsp-det 越しに
- 走行 B: `vue-language-server --stdio` を `initializationOptions` `{"typescript": {"tsdk": "<node_modules>/typescript/lib"}, "vue": {"hybridMode": true}}` で。`didOpen src/B.vue`、`references`

## 結果

### 走行 A（tsls + plugin、直接）

| 時刻（秒）   | 出来事                                                                                          |
| ------------ | ----------------------------------------------------------------------------------------------- |
| 0.062        | `initialize` 応答。`$/typescriptVersion` 5.9.3（workspace）                                     |
| 0.208        | "Initializing JS/TS language features…" begin                                                   |
| 0.393〜0.497 | `references` が **空配列を 4 回**（M6 で測った tsls の嘘と同じ）                                |
| 0.536        | 同 end                                                                                          |
| 0.651        | `references` が 4 件（`c.ts` の import と呼び出し、**`B.vue` の import と呼び出し**）。以後同じ |

`.vue` の中の参照は plugin を載せた tsserver が返す。plugin なしの tsls は `.vue` を知らない。

### 走行 A'（lsp-det 越し）

lsp-det は起動ログの名乗り（"Using Typescript version (workspace) 5.9.3"）で M6 の写像を選び、`references`（id 2）を `initializing` の間保留し（stderr "holding textDocument/references (id 2) while …"）、トークンの begin で `indexing`、end（0.416 秒）で `ready` にして解放。**最初の答えが 4 件で `B.vue` を含む。** 空応答は 1 度も通らない。

### 走行 B（vue-language-server、Hybrid Mode）

- `serverInfo` は `{"name": "@vue/language-server", "version": "3.3.10"}`。`capabilities.experimental` に `autoInsertionProvider` 等。`$/progress` も readiness のログもない
- `didOpen src/B.vue` の直後に、サーバーがクライアントへ **`tsserver/request` 通知**（`[[1, "_vue:projectInfo", {"file": ".../B.vue", "needFileNameList": false}]]`）を送る。これはクライアントが typescript-language-server の接続へ転送し（`workspace/executeCommand` "typescript.tsserverRequest"）、`tsserver/response` で返す約束（Volar の Hybrid Mode の設計）
- クライアント（probe）が転送しないと、`references` は **30 秒たっても応答がない**（空応答でも拒否でもなく、待ち続ける）

vue-language-server の接続で横断の要求は答えられず、答えは tsls の接続の完全性に従う。

## 結論（決定 B-5 の確認）

- 「接続ごとの保留で結果が完全になる」は成り立つ。横断の答えは 1 つの接続（tsls + plugin）から出て、その接続の lsp-det の保留が空応答を消す。vue-language-server の接続は横断を持たないので、保留すべきものがない
- 合成（2 つの `ServerState` の AND、`tsserver/request` の転送）はクライアントの責務で、lsp-det は隣の接続を知らない。仕様の変更は要らない。`docs/vision.md` に「相方サーバーとの合成はクライアントの責務。接続ごとの保留で足りる（M13 で実測）」と書く
- vue-language-server は写像を書かない（両軸 `unknown`）。`serverInfo` があり名乗るので、lsp-det は「知らない名前」として両軸 `unknown` を宣言する
