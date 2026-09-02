{
  # 開発環境の固定。lsp-det は準拠テストを通した言語サーバーの版にだけ保証を
  # 宣言する（src/adapter/{rust_analyzer,gopls}.rs の TESTED_VERSIONS）ので、
  # その版をここで固定し、実測が再現できるようにする。
  #
  # flake.lock を更新して言語サーバーの版が変わったら、実サーバーの結合テスト
  # （cargo test --test conformance -- --ignored）を通してから TESTED_VERSIONS を
  # 動かすこと。守れない保証の宣言は仕様 5.1 違反である。
  description = "lsp-det の開発環境（Rust ツールチェーン + rust-analyzer、go + gopls）";

  inputs = {
    # システム構成（~/nix/nixfiles）と同じ rev
    nixpkgs.url = "github:NixOS/nixpkgs/e8be7818e19ada32105a8af937a6a473b38167ca";
  };

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          rustc
          cargo
          clippy
          rustfmt
          rust-analyzer # `rust-analyzer 2026-08-03` と名乗る（rustup 版とは別ビルド）
          go
          gopls
        ];
      };
    };
}
