#!/usr/bin/env python3
"""为现有 prepare-inputs.sh 提供灵云结构分类及 IDS 分歧清单。"""

import argparse
from pathlib import Path
import re
import subprocess


IDC = set("⿰⿱⿲⿳⿴⿵⿶⿷⿸⿹⿺⿻⿼⿽⿾⿿㇯")


def read_ids_file(root, relative):
    """只使用 IDS 仓库中已被 Git 跟踪、名称不含 output 的数据文件。"""
    path = root / relative
    if any("output" in part for part in path.parts):
        raise ValueError(f"禁止读取 output 文件：{path}")
    subprocess.run(
        ["git", "-C", str(root), "ls-files", "--error-unmatch", relative],
        check=True, stdout=subprocess.DEVNULL,
    )
    return path.read_text(encoding="utf-8-sig").splitlines()


def outer(ids):
    # BabelStone 的 〾 标记不是结构描述符。
    return ids.lstrip("〾")[0]


def category(ids):
    return "lr" if outer(ids) in "⿰⿲" else "other"


def babelstone(root):
    result = {}
    for line in read_ids_file(root, "babelstone/IDS.TXT"):
        fields = line.split("\t")
        if len(fields) < 3 or len(fields[1]) != 1:
            continue
        options = []
        for field in fields[2:]:
            match = re.fullmatch(r"\^(.+)\$(?:\(([^)]*)\))?", field)
            if match:
                options.append((match[1], match[2] or ""))
        if options:
            result[fields[1]] = options
    return result


def select_ids(char, options):
    preferred = [ids for ids, tags in options if "G" in tags]
    if not preferred:
        preferred = [ids for ids, tags in options if not tags]
    if not preferred:
        preferred = [ids for ids, _ in options]
    if len({category(ids) for ids in preferred}) != 1:
        raise ValueError(f"{char} 的适用字形结构不明确：{options}")
    return preferred[0]


def alternatives(root):
    result = {"baiyi": {}, "chise": {}}
    for line in read_ids_file(root, "baiyi/ids_lv0.txt"):
        fields = line.split("\t")
        if len(fields) > 1 and len(fields[0]) == 1 and fields[1]:
            result["baiyi"][fields[0]] = fields[1].split(";")[0]
    for line in read_ids_file(root, "chise/IDS-UCS-Basic.txt"):
        fields = line.split("\t")
        if len(fields) > 2 and len(fields[1]) == 1 and fields[2]:
            result["chise"][fields[1]] = fields[2]
    return result


def prepare(ids_root, division, structures, conflicts):
    primary = babelstone(ids_root)
    others = alternatives(ids_root)
    rows, differences = [], []
    for line in division.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        char, roots, _freq = line.split("\t")
        count = len(roots.split())
        options = primary.get(char, [])
        if not options:
            if count in (2, 3):
                raise ValueError(f"二／三根字 {char} 缺少 BabelStone IDS，需人工确认")
            rows.append(f"{char}\tunused\t\t{count}")
            continue
        ids = select_ids(char, options)
        # 无描述符且不是字本身的未知记录，不应默认为“其他结构”。
        if count in (2, 3) and outer(ids) not in IDC and ids != char:
            raise ValueError(f"{char} 的 IDS 无法确定结构：{ids}")
        kind = category(ids)
        rows.append(f"{char}\t{kind}\t{ids}\t{count}")
        variants = {name: data.get(char, "") for name, data in others.items()}
        all_ids = [seq for seq, _ in options] + [v for v in variants.values() if v]
        if any(category(seq) != kind for seq in all_ids):
            differences.append("\t".join([
                char, str(count), "yes" if count in (2, 3) else "no", kind,
                ids, variants["baiyi"], variants["chise"],
                ";".join(f"{seq}({tags})" for seq, tags in options),
            ]))
    # 所有字检查通过后才写入，避免失败时留下不完整分类表。
    structures.write_text("# char\tcategory\tIDS\troot_count\n" + "\n".join(rows) + "\n", encoding="utf-8")
    conflicts.write_text(
        "# char\troot_count\taffects_code\tcategory\tselected_IDS\tbaiyi\tchise\tbabelstone_variants\n"
        + "\n".join(differences) + "\n", encoding="utf-8",
    )
    print(f"灵云：已分类 {len(rows)} 字，IDS 分类分歧 {len(differences)} 字，详见 {conflicts}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ids-root", type=Path, default=Path.home() / "home/IDS")
    parser.add_argument("--division", type=Path, default=Path("chaifen.txt"))
    parser.add_argument("--structures", type=Path, default=Path("structures-lingyun.tsv"))
    parser.add_argument("--conflicts", type=Path, default=Path("structure-conflicts-lingyun.tsv"))
    args = parser.parse_args()
    try:
        prepare(args.ids_root.expanduser(), args.division, args.structures, args.conflicts)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"灵云结构准备失败：{error}\n")


if __name__ == "__main__":
    main()
