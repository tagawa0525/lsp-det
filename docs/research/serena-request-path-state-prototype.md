# Serena の要求経路に状態を持たせる提案の検証（2026-09-15）

`docs/upstream-submissions.md` の「Serena: #1988 への返信（草案）」で提案する方法——adapter がサーバーの最新の ready / broken の状態を持ち、基底クラスの要求経路が毎回の横断要求でそれを見る——で、Serena 自身の issue（oraios/serena#1937 等）の症状が実際に消えることを、返信を出す前に試作と再現で確かめる（ユーザーの決定 2026-09-15 JST。読んだだけの主張は #2003 / #2004 で却下された）。

検証済みだったのは #2007 の 1 件（tsserver を SIGKILL した後の `references` が `[]` を成功として返す → latch の前で検査すれば例外。[serena-integration-measurement.md](serena-integration-measurement.md)）だけで、#1937 / #1858 / #1923 / #1978 は「同じ形と読んだ」に留まっていた。本報告は #1937 を再現し、試作で消えるかを測る。

## 結論

- **#1937 は再現した**。leaf に自前の tsconfig がなく、root に solution の tsconfig もなく、全パッケージを import する app のファイルを開いた状態で、`find_referencing_symbols` と同じ経路（`request_referencing_symbols`。参照先のファイルを open / close する）で同じシンボルを連続して問うと、**直前の呼び出しから 100 ms 以内の呼び出しは 13 ファイル中 1 ファイル**しか返さない。Serena main `403ad0a5` の直接の経路で、+0 ms の呼び出し 30 回中 30 回。tsls 5.3.0 + TS 5.9.3 と、#1937 と同じ tsls 5.1.3 + TS 6.0.3 の両方で再現した
- **試作（adapter の既存の状態 `_active_progress_tokens` を後続の横断要求でも見て、空でなければ `wait_for_indexing`。8 行）で +0 ms の呼び出し 36 回中 35 回が完全になる**（6 run。最初の 5 run では 30 回中 29 回）。残り 1 回は、要求の `didOpen` の数 ms 後に届く progress の begin より Serena の検査が先に走った競合
- この残りは、クライアント側の状態では閉じられない。サーバーが要求を受けた時点で自分の状態を知っていて、答える前に待つ（Dart、jdtls の形）か、状態を報告する（提案するプロトコル）かでしか閉じない。写像は橋であり、到達点はサーバー自身の報告、という主張がそのまま実測に現れた
- lsp-det を挟むと同じ 11 回が全部完全だった（保留は `indexing` の間 12〜36 ms）。lsp-det も同じ begin を見ているので同じ競合を持つはずで、1 回の run で「効く」とは言えない
- 第三者の修正 PR oraios/serena#1978（後続の問い合わせで active な token を待つ）も、この再現では 9 回とも完全。方向は試作と同じ。ただし base が古く、1 回目の挙動が main と違う（下）
- **#1858（Metals）の後続の変種も実在するが、形が違う**。新しいファイルを作って開いた直後（+0 ms）の `references` は古い答え（新しい参照を含まない）で、+1 s で正しい。Metals はその時点で何も始めておらず（最初の progress は didOpen の 135 ms 後）、状態を見る試作（V1）は捕まえない。自分の open の後に作業の開始を 1 秒まで待つ先読み（V2）なら捕まえるが、作業が来ない呼び出しに毎回 1 秒払う。これは readiness ではなく **freshness**（変更を織り込む前の古い答え）の窓で、タイマーなしに閉じられるのはサーバーだけ

## #1937 の形

cbartens の報告（2026-08-26。Serena 1.7.0、typescript-language-server 5.1.3、TypeScript 6.0.3、pnpm の monorepo、1406 ファイル、project references あり）: leaf パッケージが export する `formatDisplay` への `find_referencing_symbols` を同じセッションで 6 回。起動待ちの後の 1 回目は 11 ファイル、+0 ms の 2・3 回目は **1 ファイル**、+2 s の 4 回目以降は 11。

## latch の出自

`_has_waited_for_cross_file_references` と既定の `sleep(2)` は upstream の `c8827191`（2025-09-18、"fix(swift-lsp): improve CI stability with enhanced delays and retry logic"）で入った。目的は Swift の CI の揺れを止める delay と retry で、横断要求の正しさの設計ではない。「一度待てば以後は索引済み」という前提はその場面には合っていて、後続の要求が新しい project の読み込みを引き起こす構造（#1937）は視野になかったと読める。ADR 0018 決定 C（Serena の待ち方には CI の都合が混ざる）の実例。

## 方法

- fixture: `scripts/serena/ts-monorepo-fixture.py`。leaf（`formatDisplay` + helper 20 本）と consumer パッケージ N 個 × M ファイル。各 consumer の先頭 2 ファイルが `formatDisplay` を import。配置の選択肢: `--layout relative|pnpm`（相対 import か、`node_modules/@fixture/leaf` の symlink 経由。pnpm で leaf に tsconfig があるときは leaf の `types` が `dist/index.d.ts` を指すので、probe の前に各パッケージを `tsc -b` して dist を作った。生成器はその旨を表示する）、`--no-solution`（root に全パッケージの `references` を持つ tsconfig を置かない）、`--leaf-no-tsconfig`（leaf に tsconfig を置かず package.json の `types` で `src/index.ts` を直接指す。leaf のファイルの所属 project が読み込み状況で揺れる）、`--app`（全 consumer の `f0` と leaf を import する `apps/web`。その project の program に入るのは各パッケージの入口 `f0` と leaf のソースだけで、`f1` 以降は入らない。#1937 の環境の apps に相当。この project から見える `formatDisplay` の参照は app 自身 + 各パッケージの `f0` の 13 ファイルで、fixture 全体の 25 ではない）
- probe: `scripts/serena/repeat-references-probe.py`。solidlsp 直接（MCP なし）。`SEQUENCE` で呼び出し間の待ちを指定（既定は #1937 の +0、+0、+2、+0、+2）。`MODE=references`（`request_references` の生の結果）か `MODE=referencing_symbols`（MCP ツールと同じ `request_referencing_symbols`）。`PREOPEN` で 6 連続の前に開いたままにするファイル、`TSLS_CMD` で tsls の版、`TARGET` で問い合わせ位置。adapter が出す `$/progress` の begin / end を時刻付きで記録
- 版: PATH の typescript-language-server 5.3.0 + TypeScript 5.9.3（flake。lsp-det が保証を宣言する組）と、#1937 と同じ typescript-language-server 5.1.3 + TypeScript 6.0.3（npm から取得。tsls 5.1.3 は `--tsserver-path` を持たないので同梱の TS を解決させる）
- Serena: upstream main `403ad0a5`（#1988 のマージコミット）。試作は reference/serena のローカルブランチ `request-path-consults-state`（`403ad0a5` + 8 行）

## 再現の探索

| #      | fixture                                                                    | 版                | MODE                    | 開いたままのファイル                       | 6 回のファイル数                                                           | 判定         |
| ------ | -------------------------------------------------------------------------- | ----------------- | ----------------------- | ------------------------------------------ | -------------------------------------------------------------------------- | ------------ |
| 1      | relative、661 ファイル、solution あり                                      | 5.3.0 / 5.9.3     | references              | なし                                       | 16 × 6                                                                     | 再現せず     |
| 2      | pnpm、1461 ファイル、solution あり                                         | 5.3.0 / 5.9.3     | references              | なし                                       | 24 × 6                                                                     | 再現せず     |
| 3      | 同                                                                         | 5.1.3 / 6.0.3     | references              | なし                                       | 24 × 6                                                                     | 再現せず     |
| 4      | 同                                                                         | 5.1.3 / 6.0.3     | referencing_symbols     | なし                                       | 24 × 6                                                                     | 再現せず     |
| 5      | pnpm、1461 ファイル、solution なし                                         | 5.1.3 / 6.0.3     | 両方                    | なし                                       | 0 × 6                                                                      | 別の形（下） |
| 6      | 実物の pnpm/pnpm（1618 ファイル、172 パッケージ、solution なし、TS 6.0.3） | 5.1.3 / 6.0.3     | 両方                    | なし                                       | 1 × 6（`PnpmError`、248 ファイルで使用）                                   | 別の形       |
| 7      | 同                                                                         | 5.1.3 / 6.0.3     | references              | `pnpm11/pnpm/src/main.ts`（61 references） | 245 × 6（1 回目 16.5 s、137 project の読み込みは応答の前）                 | 再現せず     |
| 8      | pnpm、solution なし、leaf-tsconfig なし、1461 ファイル                     | 5.1.3 / 6.0.3     | 両方                    | なし                                       | 0 × 6                                                                      | 別の形       |
| 9      | 同 + app（1462 ファイル）                                                  | 5.1.3 / 6.0.3     | references              | `apps/web/src/main.ts`                     | 13 × 6                                                                     | 再現せず     |
| **10** | **同**                                                                     | **5.1.3 / 6.0.3** | **referencing_symbols** | **`apps/web/src/main.ts`**                 | **13, 1, 1, 13, 1, 13**（13 = app の project から見える全部。25 ではない） | **再現**     |
| 11     | 同                                                                         | 5.3.0 / 5.9.3     | referencing_symbols     | 同                                         | 13, 1, 1, 13, 1, 13, 1, 13, 1, 13, 1                                       | 再現         |

探索の #1〜4、7 では tsserver が referencing project を**最初の要求の中で同期的に**全部読み込んでから答える（`$/progress` が 9〜137 本、応答はその後）。#5、6、8 は tsserver が consumer の project を知る手段がなく（solution も、開かれたファイルもない）、時間が経っても直らない安定した嘘（0 件、または自分自身の 1 件）。これは #1937 の一過性の窓ではなく、Serena の待ち・latch・状態のどれを直しても変わらない。lsp-det の写像も同じ信号しか見ないので、この配置では `ready` のまま不完全な答えを通す（clangd の compilation database なしと同じ構造。信号がないことを読んで `unknown` にする余地があるかは別途）。

## 再現した配置の機構

探索の #10 / #11 の時系列（Serena のログの ms）:

1. 呼び出し 1: leaf を open → `references` → app の project の読み込み（1 本の progress）→ 13 件 → `request_referencing_symbols` が参照先 13 ファイルを open（pkg の project 12 個を読み込み。12 本の progress）→ close
2. 呼び出し 2（+0 ms）: leaf を open → `references` → **1 件**（`apps/web/src/main.ts` だけ）。この間、Serena のログに progress は**ない**
3. 呼び出し 4（+2 s）: 13 件。その後の open で pkg の project 12 個を**再び**読み込み（close の後に tsserver が解放していた）

`MODE=references`（open / close を伴わない）では #9 のとおり 13 で安定するので、揺れを起こしているのは参照先ファイルの open / close と、それに続く tsserver 内部の project の解放・再構成。窓の長さは間隔を変えて測ると **100 ms 未満**（+0.03 / 0.05 / 0.08 s は 1 件、+0.1 s は 13 件）。報告者の環境では project が重いぶん窓が長く、MCP 経由の +0 ms でも当たったと読める。

## 試作と結果

試作の差分（`typescript_language_server.py` の `_wait_for_cross_file_references_if_needed`。latch が立った後の分岐に 8 行）:

```python
if self._has_waited_for_cross_file_references:
    with self._progress_lock:
        loading = bool(self._active_progress_tokens)
    if loading:
        log.info("TypeScript is loading projects; waiting before the cross-file request (%s)", self.describe_indexing_state())
        self.wait_for_indexing(timeout=self._get_indexing_timeout())
    return
```

adapter が既に持つ状態（`_active_progress_tokens`）を、最初の 1 回だけでなく毎回の横断要求で見る、というだけの変更。#11 と同じ配置・列（+0 ms の呼び出し 6 回を含む 11 回）で:

| 版            | run | 11 回のファイル数                             | 待ちの発動 |
| ------------- | --- | --------------------------------------------- | ---------- |
| 5.3.0 / 5.9.3 | 1   | 13 × 11                                       | 6          |
| 同            | 2   | 13 × 11                                       | 6          |
| 同            | 3   | 13 × 11                                       | 6          |
| 同            | 4   | 13, 13, 13, 13, 13, 13, **1**, 13, 13, 13, 13 | 5          |
| 同            | 5   | 13 × 11                                       | 6          |
| 5.1.3 / 6.0.3 | 1   | 13 × 11                                       | 6          |

+0 ms の呼び出し 36 回中 35 回が完全。コスト: 待ちが発動した +0 ms の呼び出しは 0.38〜0.43 s（pkg の project 12 個の再読み込みを待つ。main は 0.01 s で誤答）、発動しない呼び出しは main と同じ 0.01〜0.02 s。試作のログでは、呼び出し 2 の leaf の open の直後（前の呼び出しの最後の end から 5 ms）に progress の begin が届き、検査がそれを見て待ち、pkg の project 12 個の再読み込みが終わってから `references` を送っている。run 4 の 1 回は、begin が届く前に検査が走った（待ちが発動せず即座に送った）。

main の直接の run で呼び出し 2 の前後に begin が**ない**ことと、試作の run で begin が**ある**ことの違いは説明できていない。同じメッセージ列を送っているので、tsserver / tsls の側で「要求が先に届くと遅延している project の再構成が走らない（または後で静かに走る）」といった内部の挙動があるはずで、確かめるには tsserver のログ（`--logVerbosity`）が要る。

## #1858（Metals）の後続の変種

oraios/serena#1858 は「セッション最初の `find_referencing_symbols` が固定 5 秒の待ちのせいで部分結果」で、Serena は最初の問い合わせの前で Metals の progress を待つ形（`MetalsProgressTracker`、静穏期間 3 秒）に直して閉じた（2026-08-13）。同じ latch（`_has_waited_for_cross_file_references`。`scala_language_server.py:751-777`）が Scala の adapter にもあるので、後続の問い合わせで同じ露出があるかを測った。道具は `scripts/serena/metals-later-query-probe.py`、fixture は scala-cli の単一プロジェクト（`project.scala`、`A.scala` の `A.target`、それを使う `B.scala`）、`nix develop .#servers` の Metals 1.6.8 + scala-cli 1.16.0。列: `references` → `A.target` を使う `C.scala` をディスクに作って didOpen → `references` を 4 回。間隔は直前の呼び出しから +0 ms、+1 s、+3 s、+6 s（open からの経過では 0、1、4、10 秒）。完全 = B と C の 2 件。

| Serena                                                | +0 ms           | +1 s 後     | さらに +3 s 後 | さらに +6 s 後 | 備考                                                                                                                         |
| ----------------------------------------------------- | --------------- | ----------- | -------------- | -------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| main `403ad0a5`                                       | **1**（B だけ） | 2           | 2              | 2              | didOpen（6.840）→ +0 の問い合わせ（6.845）→ Metals の "Importing build" begin（6.980）→ "Compiling"、"Indexing" end（7.444） |
| 試作 V1: 進行中の作業があれば待つ                     | **1**           | 2           | 2              | 2              | +0 の時点で `_active` は空（"reported no work"）。状態を見ても捕まらない                                                     |
| 試作 V2: 1 秒以内に作業が始まれば静穏（3 秒）まで待つ | 2（3.62 s）     | 2（1.01 s） | 2（1.01 s）    | 2（1.01 s）    | +0 は Importing → Compiling → Indexing → 静穏を待って正しい。作業が来ない呼び出しは毎回 1 秒の grace を払う                  |

読み: 露出は実在する（新しいファイルを開いた直後の問い合わせは古い答え）。ただし #1937 と違い、問い合わせの時点でサーバーはまだ何も始めていないので、adapter の readiness の状態を見ても捕まらない。捕まえるには「自分の変更の後は作業が始まるまで待つ」という先読みが要り、それは作業が来ないときの上限（タイマー）を必ず伴う。lsp-det の Metals 写像はこれを `didChangeWatchedFiles` の Created / Changed を起点にした先読み（次の "Indexing" / "Compiling" の end まで `indexing`。タイマーなし。ADR 0014 追補）で扱っているが、Serena の直接の経路は didOpen だけで通知を送らないので、その形は当てはまらない。サーバーが要求を受けた時点で「取り込み待ちの変更がある」と知っていて待つのが、タイマーなしに閉じる唯一の形。

## lsp-det と #1978

- lsp-det（main `eab5f78` の release ビルド）を `ls_base_cmd` で挟み、Serena main の直接と同じ列: 11 回とも 13。lsp-det のログでは +0 ms の要求を `indexing` の間 12〜36 ms 保留し、`ready` で解放している。begin を見て待つ点で試作と同じ機構で、同じ競合を持つはず。1 回の run で「効く」とは言えない
- oraios/serena#1978 の枝（`85fe34a6`。後続の問い合わせで active な token があれば `wait_for_indexing`）: 9 回とも 13。ただし base が `403ad0a5` より古く（59 ファイルの差分）、呼び出し 1 で pkg の project が読み込まれず呼び出し 2 で読み込まれるなど、1 回目の挙動が main と違う。同じ土俵の比較ではない

## 読み

- 提案する方法（adapter の状態を要求のたびに見る）は、再現した #1937 の形を **36 回中 35 回**消す。「同じ形と読んだ」から「測って消えた」になった
- 残る 1 回は、サーバーの信号が要求の直後に届く競合で、クライアント側の状態では原理的に閉じられない。サーバーが要求を受けた時点で自分の状態を知っていて待つ、または状態を報告する、が要る。これは提案するプロトコルの側の主張で、この実測はその根拠になる
- #1858（Metals）の後続の変種は測った（上）。readiness でなく freshness の窓で、状態を見る提案では直らず、先読みか、サーバー側でしか閉じない。返信には「同じ形」ではなく、この区別込みで書く
- #1923（Vue）は測っていない。adapter の信号が誤っている（偽の ready）形で、状態を毎回見ても直らない。返信からは外す
- solution も開かれたファイルもない配置（#5、6、8）の安定した嘘は、この提案の外。lsp-det の tsls の写像にとっても反例で、別途扱う

## 一般化してはならない点

- fixture は生成した軽量な作業ツリーで、型は単純。窓の長さ（100 ms 未満）は規模で変わる
- MCP は挟んでいない。#1937 は MCP 経由の観測で、ツール呼び出しの往復（約 200 ms）があっても報告者の環境では窓に当たっている
- 試作は tsls と Scala の adapter に置いた（Scala は環境変数で切り替える実験用）。基底クラスに置く形（提案の本体）は書いていない
- main と試作で tsserver 側の begin の有無が違う理由は未解明

## 次

- 返信の文面は測ったことに合わせて直した（[../upstream-submissions.md](../upstream-submissions.md) の草案。未提出）
- 試作の枝は fork に push 済み: https://github.com/tagawa0525/serena/tree/request-path-consults-state（`b7a7093d`、`403ad0a5` + tsls の 8 行）。Scala の V1 / V2 は reference/serena のローカル枝 `request-path-consults-state-experiment` に残す
- lsp-det の tsls 写像の反例（solution なし配置で `ready` のまま不完全）は、仕様 8.1 の `unknown` を使う余地（信号が来ない workspace と観測者が判断できるか）を含めて別の報告と ADR で扱う。仕様・写像は勝手に変えない
