"""生成 NSIS 品牌位图；仅开发时依赖 Pillow，CI 直接使用仓库中的 24 位 BMP。

沿用 src/assets/termrill.svg 的 T 字与波纹形状，不修改应用图标。
字体由 --font 指定，默认使用 Windows Segoe UI；不随仓库分发系统字体。
"""

import argparse
import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


ROOT = Path(__file__).resolve().parents[1]
SCALE = 4


def font(size, path):
    return ImageFont.truetype(path, size * SCALE)


def logo(draw, x, y, size, color):
    """按现有矢量标志的 1024 坐标绘制，超采样后统一缩小。"""
    def point(a, b):
        return ((x + a * size / 1024) * SCALE, (y + b * size / 1024) * SCALE)

    shape = [(252, 280), (772, 280), (772, 390), (570, 390), (570, 696),
             (454, 696), (454, 390), (252, 390)]
    draw.polygon([point(a, b) for a, b in shape], fill=color)
    wave = []
    for curve in [((280, 775), (400, 690), (500, 860), (620, 775)),
                  ((620, 775), (675, 735), (720, 742), (760, 762))]:
        for step in range(41):
            t = step / 40
            a = sum(math.comb(3, i) * (1 - t) ** (3 - i) * t ** i * curve[i][0]
                    for i in range(4))
            b = sum(math.comb(3, i) * (1 - t) ** (3 - i) * t ** i * curve[i][1]
                    for i in range(4))
            wave.append(point(a, b))
    stroke = max(1, round(58 * size / 1024 * SCALE))
    draw.line(wave, fill='#a8f0e1', width=stroke, joint='curve')
    for px, py in wave:
        radius = stroke / 2
        draw.ellipse((px - radius, py - radius, px + radius, py + radius), fill='#a8f0e1')


def generate(font_path):
    output = ROOT / 'src-tauri' / 'installer'
    output.mkdir(exist_ok=True)
    sidebar = Image.new('RGB', (164 * SCALE, 314 * SCALE))
    draw = ImageDraw.Draw(sidebar)
    for y in range(sidebar.height):
        t = y / (sidebar.height - 1)
        color = tuple(round(a + (b - a) * t) for a, b in zip((114, 96, 231), (53, 43, 130)))
        draw.line((0, y, sidebar.width, y), fill=color)
    logo(draw, 15, 28, 134, 'white')
    draw.text((82 * SCALE, 163 * SCALE), 'TermRill', font=font(23, font_path),
              anchor='mt', fill='white')
    draw.text((82 * SCALE, 197 * SCALE), 'SSH & SFTP', font=font(10, font_path),
              anchor='mt', fill='#d9d3ff')
    draw.line((26 * SCALE, 255 * SCALE, 138 * SCALE, 255 * SCALE),
              fill='#8273c2', width=SCALE)
    draw.text((82 * SCALE, 272 * SCALE), 'LOCAL-FIRST', font=font(8, font_path),
              anchor='mt', fill='#d9d3ff')
    sidebar.resize((328, 628), Image.Resampling.LANCZOS).save(output / 'sidebar.bmp')

    header = Image.new('RGB', (150 * SCALE, 57 * SCALE), 'white')
    draw = ImageDraw.Draw(header)
    draw.rounded_rectangle((10 * SCALE, 10 * SCALE, 47 * SCALE, 47 * SCALE),
                           radius=9 * SCALE, fill='#6755d8')
    logo(draw, 10, 10, 37, 'white')
    draw.text((56 * SCALE, 19 * SCALE), 'TermRill', font=font(17, font_path), fill='#403765')
    header.resize((300, 114), Image.Resampling.LANCZOS).save(output / 'header.bmp')
    print('Generated 24-bit installer artwork: sidebar.bmp, header.bmp')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--font', default='segoeuib.ttf', help='Segoe UI Bold TTF path')
    generate(parser.parse_args().font)
