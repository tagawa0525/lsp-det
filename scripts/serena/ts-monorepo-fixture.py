#!/usr/bin/env python3
"""oraios/serena#1937 の形を再現するための TypeScript の monorepo を生成する。

leaf パッケージが `formatDisplay` を export し、複数の consumer パッケージの一部のファイルが
それを import する。残りのファイルは package 内の相互 import と leaf の helper の import だけで、
tsserver に読み込ませるプロジェクトを大きくするための嵩。

    ts-monorepo-fixture.py <dir> [--packages 8] [--files 80] [--consumers 2]
        [--layout relative|pnpm] [--no-solution] [--leaf-no-tsconfig] [--app]

--layout relative  consumer が `../../leaf/src/index` を相対 import する
--layout pnpm      pnpm workspace の形。consumer は `@fixture/leaf` を import し、それは
                   `node_modules/@fixture/leaf` の symlink で `packages/leaf` に解決される。leaf の
                   package.json の `types` は `dist/index.d.ts` (tsc -b で dist を作る)
--no-solution      root に solution の tsconfig.json (全パッケージの references) を置かない。pnpm の
                   monorepo に多い形。tsserver は leaf から referencing project を辿れない
--leaf-no-tsconfig leaf に tsconfig を置かず、package.json の types で src/index.ts を直接指す。
                   consumer の tsconfig は leaf を references しない。leaf のファイルは「そのとき
                   読み込まれている consumer の project」に属し、なければ inferred project になる
--app              全 consumer の f0 と leaf を import する app (apps/web) を足す。その project の
                   program に入るのは各パッケージの入口 f0 と leaf のソースだけ (f1 以降は入らない)。
                   #1937 の環境の apps に相当し、この project から見える formatDisplay の参照は
                   app 自身 + 各パッケージの f0 の N+1 ファイル

出力の最後に、`formatDisplay` の期待する参照ファイル数 (consumer のファイル数。Serena は宣言を
含めない。--app なら +1) を表示する。
"""

import argparse
import json
import os
import shutil

LEAF_FILES = 20


def write(path: str, text: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def tsconfig(references: list[str]) -> str:
    return json.dumps(
        {
            "compilerOptions": {
                "composite": True,
                "declaration": True,
                "strict": True,
                "module": "esnext",
                "moduleResolution": "bundler",
                "target": "es2022",
                "rootDir": "src",
                "outDir": "dist",
            },
            "include": ["src"],
            "references": [{"path": r} for r in references],
        },
        indent=2,
    )


def link(package_dir: str, target_dir: str, name: str) -> None:
    link_dir = os.path.join(package_dir, "node_modules", "@fixture")
    os.makedirs(link_dir, exist_ok=True)
    os.symlink(
        os.path.relpath(target_dir, link_dir),
        os.path.join(link_dir, name),
        target_is_directory=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("dir")
    parser.add_argument("--packages", type=int, default=8)
    parser.add_argument("--files", type=int, default=80)
    parser.add_argument(
        "--consumers",
        type=int,
        default=2,
        help="formatDisplay を import するファイル数 (package ごと)",
    )
    parser.add_argument("--layout", choices=["relative", "pnpm"], default="relative")
    parser.add_argument("--no-solution", action="store_true")
    parser.add_argument("--leaf-no-tsconfig", action="store_true")
    parser.add_argument("--app", action="store_true")
    parser.add_argument(
        "--force",
        action="store_true",
        help="<dir> が既にあれば消して作り直す (既定では止まる)",
    )
    args = parser.parse_args()
    if args.packages < 1 or args.files < 1:
        parser.error("--packages and --files must be positive")
    if not 1 <= args.consumers <= args.files:
        parser.error("--consumers must be between 1 and --files")
    root = os.path.abspath(args.dir)
    if os.path.lexists(root):  # 壊れた symlink も含めて、あれば止まる
        if not args.force:
            parser.error(
                f"{root} already exists; pass --force to replace it (it will be deleted)"
            )
        shutil.rmtree(root)
    os.makedirs(root)
    pnpm = args.layout == "pnpm"
    leaf_import = "@fixture/leaf" if pnpm else "../../leaf/src/index"

    # leaf
    leaf = os.path.join(root, "packages", "leaf")
    if args.leaf_no_tsconfig:
        leaf_pkg = {
            "name": "@fixture/leaf",
            "version": "0.0.0",
            "main": "src/index.ts",
            "types": "src/index.ts",
        }
    else:
        write(os.path.join(leaf, "tsconfig.json"), tsconfig([]))
        leaf_pkg = {
            "name": "@fixture/leaf",
            "version": "0.0.0",
            "main": "dist/index.js",
            "types": "dist/index.d.ts",
        }
    write(os.path.join(leaf, "package.json"), json.dumps(leaf_pkg, indent=2))
    write(
        os.path.join(leaf, "src", "index.ts"),
        "export function formatDisplay(value: number): string {\n  return `#${value}`;\n}\n"
        + "".join(
            f'export {{ helper{i} }} from "./helper{i}";\n' for i in range(LEAF_FILES)
        ),
    )
    for i in range(LEAF_FILES):
        write(
            os.path.join(leaf, "src", f"helper{i}.ts"),
            f"export function helper{i}(x: number): number {{\n  return x + {i};\n}}\n",
        )

    # consumers
    names = [f"pkg{n}" for n in range(args.packages)]
    consumers = 0
    for name in names:
        pkg = os.path.join(root, "packages", name)
        write(
            os.path.join(pkg, "tsconfig.json"),
            tsconfig([] if args.leaf_no_tsconfig else ["../leaf"]),
        )
        write(
            os.path.join(pkg, "package.json"),
            json.dumps(
                {"name": f"@fixture/{name}", "version": "0.0.0", "types": "src/f0.ts"},
                indent=2,
            ),
        )
        if pnpm:
            link(pkg, leaf, "leaf")
        for k in range(args.files):
            prev = f'import {{ f{k - 1} }} from "./f{k - 1}";\n' if k > 0 else ""
            if k < args.consumers:
                body = (
                    f'import {{ formatDisplay }} from "{leaf_import}";\n'
                    + prev
                    + f"export function f{k}(): string {{\n  return formatDisplay({k});\n}}\n"
                )
                consumers += 1
            else:
                h = k % LEAF_FILES
                body = (
                    prev
                    + f'import {{ helper{h} }} from "{leaf_import}";\n'
                    + f"export function f{k}(): number {{\n  return helper{h}(String(f{k - 1}()).length);\n}}\n"
                )
            write(os.path.join(pkg, "src", f"f{k}.ts"), body)

    # app
    if args.app:
        app = os.path.join(root, "apps", "web")
        write(os.path.join(app, "tsconfig.json"), tsconfig([]))
        write(
            os.path.join(app, "package.json"),
            json.dumps(
                {"name": "@fixture/web", "version": "0.0.0", "private": True}, indent=2
            ),
        )
        for name in names:
            link(app, os.path.join(root, "packages", name), name)
        link(app, leaf, "leaf")
        write(
            os.path.join(app, "src", "main.ts"),
            "".join(f'import {{ f0 as {n}_f0 }} from "@fixture/{n}";\n' for n in names)
            + 'import { formatDisplay } from "@fixture/leaf";\n'
            + "export const all = ["
            + ", ".join(f"{n}_f0()" for n in names)
            + ", formatDisplay(0)];\n",
        )
        consumers += 1
        visible_from_app = args.packages + 1  # app 自身 + 各パッケージの入口 f0

    # root
    if not args.no_solution:
        # leaf に tsconfig がなければ references に入れられない (tsc は各 path を tsconfig.json として読む)
        refs = ([] if args.leaf_no_tsconfig else [{"path": "packages/leaf"}]) + [
            {"path": f"packages/{n}"} for n in names
        ]
        if args.app:
            refs.append({"path": "apps/web"})
        write(
            os.path.join(root, "tsconfig.json"),
            json.dumps({"files": [], "references": refs}, indent=2),
        )
    write(
        os.path.join(root, "package.json"),
        json.dumps({"name": "fixture", "private": True}, indent=2),
    )
    if pnpm:
        write(
            os.path.join(root, "pnpm-workspace.yaml"),
            "packages:\n  - packages/*\n  - apps/*\n",
        )
    total = LEAF_FILES + 1 + args.packages * args.files + (1 if args.app else 0)
    if pnpm and not args.leaf_no_tsconfig:
        # leaf の types は dist を指すので、tsserver に解決させるには先にビルドが要る
        print(
            f"note: build dist first, e.g. `for p in '{root}'/packages/*/; do tsc -b \"$p\"; done` (leaf's types point at dist/index.d.ts)"
        )
    print(
        f"{root}: layout={args.layout}, solution={'no' if args.no_solution else 'yes'}, "
        f"leaf-tsconfig={'no' if args.leaf_no_tsconfig else 'yes'}, app={'yes' if args.app else 'no'}, "
        f"{total} .ts files, formatDisplay declared in packages/leaf/src/index.ts:0:16, "
        f"expected reference files = {consumers}"
        + (
            f" in the whole fixture; visible from the app project = {visible_from_app}"
            if args.app
            else ""
        )
    )


if __name__ == "__main__":
    main()
