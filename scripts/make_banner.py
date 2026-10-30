#!/usr/bin/env python3
"""
Generate Flagship Light-Themed README Hero Banner using actual application screenshot.
"""

import math
from PIL import Image, ImageDraw, ImageFont, ImageFilter

BANNER_W = 1920
BANNER_H = 860

def get_font(size, bold=True, zh=False):
    if zh:
        return ImageFont.truetype("/System/Library/Fonts/Hiragino Sans GB.ttc", size)
    return ImageFont.truetype("/System/Library/Fonts/HelveticaNeue.ttc", size)

def make_banner():
    # Light theme gradient background: porcelain white to light ice-blue/emerald
    base = Image.new("RGBA", (BANNER_W, BANNER_H), (246, 248, 251, 255))
    draw = ImageDraw.Draw(base)

    # Subtle ambient gradient washes
    # Top-left mint glow, top-right indigo glow
    for y in range(BANNER_H):
        t = y / float(BANNER_H)
        r = int(246 * (1 - t) + 240 * t)
        g = int(248 * (1 - t) + 244 * t)
        b = int(252 * (1 - t) + 250 * t)
        draw.line([0, y, BANNER_W, y], fill=(r, g, b, 255))

    # Grid pattern
    grid_col = (235, 238, 243, 255)
    for x in range(0, BANNER_W, 60):
        draw.line([x, 0, x, BANNER_H], fill=grid_col, width=1)
    for y in range(0, BANNER_H, 60):
        draw.line([0, y, BANNER_W, y], fill=grid_col, width=1)

    # Load actual program screenshot
    screen_path = "docs/screenshots/01_changes_diff_light.png"
    screen = Image.open(screen_path).convert("RGBA")

    # Resize screenshot to fit nicely in the lower portion of the banner
    # Original is 1440x900
    target_w = 1460
    target_h = int(screen.height * (target_w / screen.width))
    screen_resized = screen.resize((target_w, target_h), Image.Resampling.LANCZOS)

    # Window container with rounded corners and drop shadow
    # Create shadow
    shadow_pad = 40
    shadow_img = Image.new("RGBA", (target_w + shadow_pad * 2, target_h + shadow_pad * 2), (0, 0, 0, 0))
    sdraw = ImageDraw.Draw(shadow_img)
    sdraw.rounded_rectangle(
        [shadow_pad, shadow_pad + 10, shadow_pad + target_w, shadow_pad + target_h + 10],
        radius=16,
        fill=(0, 0, 0, 45)
    )
    shadow_img = shadow_img.filter(ImageFilter.GaussianBlur(24))

    # Paste shadow
    win_x = (BANNER_W - target_w) // 2
    win_y = 260
    base.paste(shadow_img, (win_x - shadow_pad, win_y - shadow_pad), shadow_img)

    # Mask the screenshot with rounded corners and a delicate border
    mask = Image.new("L", (target_w, target_h), 0)
    mdraw = ImageDraw.Draw(mask)
    mdraw.rounded_rectangle([0, 0, target_w, target_h], radius=14, fill=255)
    screen_resized.putalpha(mask)

    # Border frame
    border_layer = Image.new("RGBA", (target_w, target_h), (0, 0, 0, 0))
    bdraw = ImageDraw.Draw(border_layer)
    bdraw.rounded_rectangle([0, 0, target_w, target_h], radius=14, outline=(210, 215, 225, 255), width=2)

    base.paste(screen_resized, (win_x, win_y), screen_resized)
    base.paste(border_layer, (win_x, win_y), border_layer)

    # Top Header Content
    # App Icon (100x100)
    icon = Image.open("assets/icon.png").convert("RGBA").resize((90, 90), Image.Resampling.LANCZOS)
    icon_x = win_x + 20
    icon_y = 65
    base.paste(icon, (icon_x, icon_y), icon)

    # Header Texts
    draw = ImageDraw.Draw(base)
    f_title = get_font(38, bold=True)
    f_zh = get_font(22, zh=True)
    f_desc = get_font(15, zh=True)
    f_pill = get_font(12, zh=True)

    text_x = icon_x + 110
    draw.text((text_x, icon_y + 4), "Nemorous", fill=(17, 24, 39), font=f_title)
    draw.text((text_x + 215, icon_y + 16), "多仓如林 · 统一俯瞰", fill=(16, 185, 129), font=f_zh)

    desc_text = "专为 AI 编程时代打造的高性能、极低内存占用、秒级启动的多仓库 Git 协同桌面工具"
    draw.text((text_x, icon_y + 54), desc_text, fill=(75, 85, 99), font=f_desc)

    # Feature badges on the right side of header
    pills = [
        ("⚡ 纯原生 GPU 渲染", (238, 242, 255), (79, 70, 229), (199, 210, 254)),
        ("🌲 零 WebView / Electron", (236, 253, 245), (16, 185, 129), (167, 243, 208)),
        ("🛡️ 6 项机器强安全校验", (254, 243, 199), (180, 83, 9), (253, 230, 138)),
        ("🚀 CGO=0 单二进制", (243, 244, 246), (55, 65, 81), (229, 231, 235)),
    ]

    px = win_x + target_w
    for ptext, pbg, pfg, pborder in reversed(pills):
        tb = f_pill.getbbox(ptext)
        pw = (tb[2] - tb[0]) + 24
        ph = (tb[3] - tb[1]) + 12
        px -= pw
        draw.rounded_rectangle([px, icon_y + 24, px + pw, icon_y + 24 + ph], radius=14, fill=pbg, outline=pborder, width=1)
        draw.text((px + 12, icon_y + 24 + 6 - tb[1]), ptext, fill=pfg, font=f_pill)
        px -= 10

    # Save
    base.convert("RGB").save("assets/banner.png", "PNG", optimize=True)
    base.convert("RGB").save("docs/img/banner.png", "PNG", optimize=True)
    print("Light-themed hero banner successfully created with actual screenshot!")

if __name__ == "__main__":
    make_banner()
