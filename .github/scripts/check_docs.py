#!/usr/bin/env python3
"""离线校验文档相对链接、源码行号锚点与示例配置结构。

只使用 Python 标准库，供 .github/workflows/docs.yml 在纯文档变更时运行。
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
# Markdown 链接：[文本](目标)
LINK_PATTERN = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
# 源码行号锚点：L24 或 L24-L30
ANCHOR_PATTERN = re.compile(r"^L(\d+)(?:-L(\d+))?$")


def markdown_documents():
    return [ROOT / "README.md", *sorted((ROOT / "docs").glob("*.md"))]


def check_links():
    errors = []
    checked = 0
    for document in markdown_documents():
        if not document.is_file():
            errors.append("文档缺失：{}".format(document.relative_to(ROOT).as_posix()))
            continue
        text = document.read_text(encoding="utf-8")
        display = document.relative_to(ROOT).as_posix()
        for target in LINK_PATTERN.findall(text):
            if target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            path_part, _, anchor = target.partition("#")
            if not path_part:
                continue
            resolved = (document.parent / path_part).resolve()
            if not resolved.exists():
                errors.append("{}: 链接目标不存在 {}".format(display, target))
                continue
            checked += 1
            if not anchor or resolved.suffix != ".rs":
                continue
            match = ANCHOR_PATTERN.match(anchor)
            if match is None:
                continue
            start = int(match.group(1))
            end = int(match.group(2) or start)
            total = len(resolved.read_text(encoding="utf-8").splitlines())
            if start < 1 or end < start or end > total:
                errors.append(
                    "{}: 行号锚点越界 {}（目标文件共 {} 行）".format(display, target, total)
                )
    return errors, checked


def check_example_config():
    """不用 YAML 库，只做关键行存在性检查。

    runner 上的 Python 不保证安装 PyYAML，这里改为按行检查示例配置的关键结构，
    避免为文档校验引入第三方依赖。
    """
    path = ROOT / "config" / "config.example.yaml"
    if not path.is_file():
        return ["示例配置缺失：config/config.example.yaml"]
    lines = path.read_text(encoding="utf-8").splitlines()
    errors = []
    for expected in ("version: 1", "accounts:", "notifications:"):
        if not any(line.strip() == expected for line in lines):
            errors.append("示例配置缺少顶层结构：{}".format(expected))
    return errors


def main():
    link_errors, checked = check_links()
    errors = link_errors + check_example_config()
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(
        "文档校验通过：{} 个文档，{} 个文件链接，示例配置结构完整".format(
            len(markdown_documents()), checked
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
