{
  # 開発環境の固定。lsp-det は準拠テストを通した言語サーバーの版にだけ保証を
  # 宣言する（src/adapter/{rust_analyzer,gopls}.rs の TESTED_VERSIONS）ので、
  # その版をここで固定し、実測が再現できるようにする。
  #
  # flake.lock を更新して言語サーバーの版が変わったら、実サーバーの結合テスト
  # （cargo test --test conformance -- --ignored）を通してから TESTED_VERSIONS を
  # 動かすこと。守れない保証の宣言は仕様 5.1 違反である。
  description = "lsp-det の開発環境。default はビルドの道具だけ、servers は言語サーバー全部（準拠テストの実サーバー結合とドッグフーディング用）";

  inputs = {
    # システム構成（~/nix/nixfiles）と同じ rev
    nixpkgs.url = "github:NixOS/nixpkgs/e8be7818e19ada32105a8af937a6a473b38167ca";
  };

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      # lsp-det をビルドし決定的なテスト（偽上流・偽クライアント）を回す道具。
      # 軽く使う人はこれだけでよい。言語サーバーは自分の環境のものを使う。
      tools = with pkgs; [
        rustc
        cargo
        clippy
        rustfmt
      ];
      # Expert (Elixir) は nixpkgs にないので release の静的バイナリを取る (ADR 0019 決定 F、M10)。
      # 起動には erl と elixir が PATH に要る。初回起動時に ~/.cache/expert にエンジンをビルドする。
      expert = pkgs.stdenv.mkDerivation {
        pname = "expert";
        version = "0.1.9";
        src = pkgs.fetchurl {
          url = "https://github.com/elixir-lang/expert/releases/download/v0.1.9/expert_linux_amd64";
          hash = "sha256-99WQW8PwmxKNSUHHdpYz8tEIvmN/Io7kMentE30sVWM=";
        };
        dontUnpack = true;
        installPhase = "install -Dm755 $src $out/bin/expert";
      };
      # Nextflow の言語サーバーは nixpkgs にないので release の jar を取る (ADR 0019 決定 F、M12)。
      # java -jar で起動する。serverInfo を返さないので版は語彙に現れない。
      nextflow-language-server = pkgs.writeShellScriptBin "nextflow-language-server" ''
        exec ${pkgs.jdk21}/bin/java -jar ${
          pkgs.fetchurl {
            url = "https://github.com/nextflow-io/language-server/releases/download/v26.04.3/language-server-all.jar";
            hash = "sha256-IM+jT24gLWuLq9jXhiAs4A4NObcMzsMpDiqz+9ArwBY=";
          }
        } "$@"
      '';
      # 準拠テストの実サーバー結合（cargo test --test conformance -- --ignored）と
      # ドッグフーディングに使う言語サーバー。版の固定はここ（ADR 0019 決定 D）。
      servers = with pkgs; [
        rust-analyzer # `rust-analyzer 2026-08-03` と名乗る（rustup 版とは別ビルド）
        go
        gopls
        pyright # M5 の写像。serverInfo を返さないので、名乗りと版は起動時の window/logMessage から読む (ADR 0011)
        basedpyright # pyright の通知名を継承。写像を共有する
        nodejs # typescript-language-server の実行環境
        pnpm # 上流 (pyright、typescript-language-server) をソースからビルドするとき (scripts/upstream/)
        typescript # tsserver 本体 (typescript-language-server が --tsserver-path なしで探す)
        typescript-language-server # M6 の写像
        metals # M9 の写像 (ADR 0019 決定 F)。scala-cli のプロジェクトを BSP で取り込む
        scala-cli # Metals のビルドツール兼 BSP サーバー
        jdk21 # Metals、scala-cli、Nextflow の言語サーバーの実行環境
        elixir # Expert が起動時に探す (M10)
        erlang # 同上
        expert # M10 の写像
        nextflow-language-server # M12 の写像
        haskell-language-server # M15 (ADR 0019 決定 F)。readiness の信号は時間で抑制されるので写像は health だけ
        ghc # haskell-language-server と同じ版でビルドされた GHC（cradle の読み込みに要る）
        cabal-install # hie-bios の cabal cradle が呼ぶ
        pyrefly # M16 (ADR 0019 決定 F)。起動時の索引は stderr にしか出ず、写像はない（両軸 unknown）
      ];
    in
    {
      devShells.${system} = {
        default = pkgs.mkShell { packages = tools; };
        servers = pkgs.mkShell { packages = tools ++ servers; };
      };
    };
}
