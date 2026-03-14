from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path


BOX_RE = re.compile(
    r"^\s*(\d+):\s+(\d+)x(\d+)\+(\d+)\+(\d+)\s+[^ ]+\s+([^ ]+)\s+gray\(0\)"
)


def load_boxes(path: Path) -> list[tuple[int, int, int, int]]:
    boxes: list[tuple[int, int, int, int]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line:
            continue
        left, top, width, height = [int(value) for value in line.split("\t")]
        boxes.append((left, top, width, height))
    return boxes


def write_draw_script(path: Path, boxes: list[tuple[int, int, int, int]]) -> None:
    lines: list[str] = []
    for left, top, width, height in boxes:
        pad = 4
        x0 = max(left - pad, 0)
        y0 = max(top - pad, 0)
        x1 = left + width + pad
        y1 = top + height + pad
        lines.append(f"rectangle {x0},{y0} {x1},{y1}")
    path.write_text("\n".join(lines), encoding="utf-8")


def run_magick(command: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, check=False, text=True, capture_output=True)


def parse_components(output: str, page_width: int, page_height: int) -> list[tuple[int, int, int, int]]:
    boxes: list[tuple[int, int, int, int]] = []
    page_area = page_width * page_height

    for line in output.splitlines():
        match = BOX_RE.match(line)
        if not match:
            continue
        object_id, width, height, left, top, area = match.groups()
        if object_id == "0":
            continue

        width_i = int(width)
        height_i = int(height)
        left_i = int(left)
        top_i = int(top)
        area_i = int(float(area))

        if width_i < 18 or height_i < 18 or area_i < 600:
            continue
        if area_i > page_area * 0.85:
            continue
        boxes.append((left_i, top_i, width_i, height_i))

    return merge_boxes(boxes, gap=12)


def merge_boxes(boxes: list[tuple[int, int, int, int]], gap: int) -> list[tuple[int, int, int, int]]:
    merged: list[tuple[int, int, int, int]] = []
    for left, top, width, height in sorted(boxes):
        right = left + width
        bottom = top + height
        expanded = (left - gap, top - gap, right + gap, bottom + gap)

        found = False
        for index, (m_left, m_top, m_width, m_height) in enumerate(merged):
            m_right = m_left + m_width
            m_bottom = m_top + m_height
            if not (
                expanded[2] < m_left
                or expanded[0] > m_right
                or expanded[3] < m_top
                or expanded[1] > m_bottom
            ):
                new_left = min(left, m_left)
                new_top = min(top, m_top)
                new_right = max(right, m_right)
                new_bottom = max(bottom, m_bottom)
                merged[index] = (new_left, new_top, new_right - new_left, new_bottom - new_top)
                found = True
                break

        if not found:
            merged.append((left, top, width, height))

    if len(merged) == len(boxes):
        return merged
    return merge_boxes(merged, gap)


def main() -> int:
    if len(sys.argv) != 5:
        print(
            "usage: extract_non_text_regions.py <PAGE_PNG> <BOXES_FILE> <OUTPUT_DIR> <PREFIX>",
            file=sys.stderr,
        )
        return 2

    page_path = Path(sys.argv[1])
    boxes_path = Path(sys.argv[2])
    output_dir = Path(sys.argv[3])
    prefix = sys.argv[4]

    output_dir.mkdir(parents=True, exist_ok=True)

    identify = run_magick(["magick", "identify", "-format", "%w %h", str(page_path)])
    if identify.returncode != 0:
        sys.stderr.write(identify.stderr)
        return 1
    width_str, height_str = identify.stdout.strip().split()
    page_width = int(width_str)
    page_height = int(height_str)

    draw_script = output_dir / f"{prefix}-mask.mvg"
    write_draw_script(draw_script, load_boxes(boxes_path))

    components = run_magick(
        [
            "magick",
            str(page_path),
            "-background",
            "white",
            "-alpha",
            "remove",
            "-alpha",
            "off",
            "-colorspace",
            "gray",
            "-threshold",
            "96%",
            "-fill",
            "white",
            "-stroke",
            "white",
            "-draw",
            f"@{draw_script}",
            "-morphology",
            "Dilate",
            "Octagon:1",
            "-define",
            "connected-components:verbose=true",
            "-define",
            "connected-components:area-threshold=600",
            "-connected-components",
            "8",
            "NULL:",
        ]
    )
    if components.returncode != 0:
        sys.stderr.write(components.stderr)
        return 1

    for index, (left, top, width, height) in enumerate(
        parse_components(components.stdout, page_width, page_height), start=1
    ):
        filename = f"{prefix}-region-{index:03d}.png"
        crop_path = output_dir / filename
        crop = run_magick(
            [
                "magick",
                str(page_path),
                "-crop",
                f"{width}x{height}+{left}+{top}",
                "+repage",
                str(crop_path),
            ]
        )
        if crop.returncode != 0:
            sys.stderr.write(crop.stderr)
            return 1
        print(f"REGION\t{left}\t{top}\t{width}\t{height}\t{crop_path}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
