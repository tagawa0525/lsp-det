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
      ];
    in
    {
      devShells.${system} = {
        default = pkgs.mkShell { packages = tools; };
        servers = pkgs.mkShell { packages = tools ++ servers; };
      };
    };
}
