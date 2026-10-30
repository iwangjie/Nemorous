#!/usr/bin/env python3
"""
Nemorous Flagship Open Source Promo Video Generator (Real Screenshots Edition)
Renders 540 frames @ 30fps (18s) Full HD (1920x1080) in Light Theme with real UI captures.
"""

import os
import math
import shutil
import subprocess
from concurrent.futures import ProcessPoolExecutor
from PIL import Image, ImageDraw, ImageFont, ImageFilter

WIDTH = 1920
HEIGHT = 1080
FPS = 30
TOTAL_FRAMES = 540  # 18 seconds

FRAME_DIR = "/tmp/nemorous_promo_frames"

FONT_MAIN_BOLD = "/System/Library/Fonts/HelveticaNeue.ttc"
FONT_ZH_BOLD = "/System/Library/Fonts/Hiragino Sans GB.ttc"
FONT_MONO = "/System/Library/Fonts/Menlo.ttc"

def get_font(size, bold=True, mono=False, zh=False):
    if mono:
        return ImageFont.truetype(FONT_MONO, size)
    if zh:
        return ImageFont.truetype(FONT_ZH_BOLD, size)
    return ImageFont.truetype(FONT_MAIN_BOLD, size)

def ease_in_out(t):
    return t * t * (3 - 2 * t)

def ease_out(t):
    return 1 - (1 - t) ** 3

def clamp(v, min_v=0.0, max_v=1.0):
    return max(min_v, min(max_v, v))

def draw_pill(draw, x, y, text, bg, fg, font, pad_x=14, pad_y=6, radius=12, border=None):
    bbox = font.getbbox(text)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    w = tw + pad_x * 2
    h = th + pad_y * 2
    draw.rounded_rectangle([x, y, x + w, y + h], radius=radius, fill=bg, outline=border, width=1 if border else 0)
    draw.text((x + pad_x, y + pad_y - bbox[1]), text, fill=fg, font=font)
    return w, h

# Load real captured screenshots
SCREENS = {
    "changes": Image.open("docs/screenshots/01_changes_diff_light.png").convert("RGBA"),
    "code": Image.open("docs/screenshots/02_code_explorer_light.png").convert("RGBA"),
    "forest": Image.open("docs/screenshots/03_commit_forest_light.png").convert("RGBA"),
    "branch": Image.open("docs/screenshots/04_branch_switch_light.png").convert("RGBA"),
}

ICON_PATH = "assets/icon.png"
APP_ICON = Image.open(ICON_PATH).convert("RGBA") if os.path.exists(ICON_PATH) else None

def render_window_mockup(screen, target_w, crop_box=None):
    """
    Renders screenshot inside a clean macOS window frame with rounded corners & shadow.
    """
    src = screen
    if crop_box:
        src = screen.crop(crop_box)

    target_h = int(src.height * (target_w / src.width))
    resized = src.resize((target_w, target_h), Image.Resampling.LANCZOS)

    # Frame with shadow
    pad = 32
    canvas = Image.new("RGBA", (target_w + pad * 2, target_h + pad * 2), (0, 0, 0, 0))
    cdraw = ImageDraw.Draw(canvas)
    # Shadow
    cdraw.rounded_rectangle([pad, pad + 8, pad + target_w, pad + target_h + 8], radius=16, fill=(0, 0, 0, 35))
    canvas = canvas.filter(ImageFilter.GaussianBlur(18))

    # Mask image with rounded corners
    mask = Image.new("L", (target_w, target_h), 0)
    mdraw = ImageDraw.Draw(mask)
    mdraw.rounded_rectangle([0, 0, target_w, target_h], radius=14, fill=255)
    resized.putalpha(mask)

    canvas.paste(resized, (pad, pad), resized)

    # Subtle border
    bdraw = ImageDraw.Draw(canvas)
    bdraw.rounded_rectangle([pad, pad, pad + target_w, pad + target_h], radius=14, outline=(215, 220, 230, 255), width=2)
    return canvas, target_w, target_h

def render_frame(f):
    # Light theme canvas: #F6F8FB with delicate geometric accents
    im = Image.new("RGBA", (WIDTH, HEIGHT), (246, 248, 251, 255))
    draw = ImageDraw.Draw(im)

    # Grid background
    grid_col = (236, 239, 245, 255)
    for gx in range(0, WIDTH, 60):
        draw.line([gx, 0, gx, HEIGHT], fill=grid_col, width=1)
    for gy in range(0, HEIGHT, 60):
        draw.line([0, gy, WIDTH, gy], fill=grid_col, width=1)

    # -------------------------------------------------------------
    # SCENE 1: TITLE & LOGO INTRO (0.0s - 3.2s, frames 0-96)
    # -------------------------------------------------------------
    if f < 96:
        p = f / 96.0
        scale = 0.85 + 0.15 * ease_out(clamp(p / 0.5))

        cx, cy = WIDTH // 2, HEIGHT // 2 - 40

        # App Icon
        if APP_ICON:
            isz = int(180 * scale)
            icon_res = APP_ICON.resize((isz, isz), Image.Resampling.LANCZOS)
            # Add icon shadow
            ishadow = Image.new("RGBA", (isz + 40, isz + 40), (0, 0, 0, 0))
            isdraw = ImageDraw.Draw(ishadow)
            isdraw.rounded_rectangle([20, 24, 20 + isz, 24 + isz], radius=40, fill=(0, 0, 0, 30))
            ishadow = ishadow.filter(ImageFilter.GaussianBlur(16))
            im.paste(ishadow, (cx - isz // 2 - 20, cy - isz // 2 - 80 - 20), ishadow)
            im.paste(icon_res, (cx - isz // 2, cy - isz // 2 - 80), icon_res)

        f_big = get_font(52, bold=True)
        t_text = "Nemorous"
        tb = f_big.getbbox(t_text)
        draw.text((cx - (tb[2] - tb[0]) // 2, cy + 60), t_text, fill=(17, 24, 39), font=f_big)

        f_sub = get_font(24, zh=True)
        s_text = "多仓如林 · 统一俯瞰"
        sb = f_sub.getbbox(s_text)
        draw.text((cx - (sb[2] - sb[0]) // 2, cy + 128), s_text, fill=(16, 185, 129), font=f_sub)

        f_en = get_font(16, zh=False)
        e_text = "The Native Git Multi-Repo Cockpit for the AI Coding Era"
        eb = f_en.getbbox(e_text)
        draw.text((cx - (eb[2] - eb[0]) // 2, cy + 168), e_text, fill=(107, 114, 128), font=f_en)

        # Feature Badges
        f_pill = get_font(13, zh=True)
        pills = [
            ("⚡ 纯原生 GPU 渲染", (238, 242, 255), (79, 70, 229), (199, 210, 254)),
            ("🌲 零 WebView / Electron", (236, 253, 245), (16, 185, 129), (167, 243, 208)),
            ("🛡️ 6 项机器强安全校验", (254, 243, 199), (180, 83, 9), (253, 230, 138)),
            ("🚀 CGO=0 单二进制跨平台", (243, 244, 246), (55, 65, 81), (229, 231, 235)),
        ]
        total_w = sum(f_pill.getbbox(p[0])[2] - f_pill.getbbox(p[0])[0] + 28 for p in pills) + 36
        px = (WIDTH - total_w) // 2
        py = cy + 220
        for ptext, pbg, pfg, pborder in pills:
            w, _ = draw_pill(draw, px, py, ptext, pbg, pfg, f_pill, border=pborder)
            px += w + 12

    # -------------------------------------------------------------
    # SCENE 2: ACTUAL CHANGES & DIFF REVIEW (3.2s - 7.0s, frames 96-210)
    # -------------------------------------------------------------
    elif f < 210:
        local_f = f - 96
        p = local_f / 114.0

        # Header Badge
        f_hdr = get_font(18, zh=True)
        draw_pill(draw, 100, 40, "FEATURE 01 · 📝 多仓变更层次树与行级微调（实际程序界面）", (238, 242, 255), (79, 70, 229), f_hdr, pad_x=18, pad_y=8, border=(199, 210, 254))

        # Floating window with actual screenshot
        # Smooth slight upward float animation
        offset_y = int(10 * (1 - ease_out(clamp(p / 0.3))))
        win_w = 1520
        win_img, ww, wh = render_window_mockup(SCREENS["changes"], win_w)
        im.paste(win_img, ((WIDTH - win_img.width) // 2, 110 + offset_y - 32), win_img)

        # Callout Annotations
        f_callout = get_font(13, zh=True)
        # Callout 1: Hierarchical file tree
        draw_pill(draw, 140, 780, "✓ 修复后的严格层级缩进：仓库 → 目录 → 文件 对齐清晰", (236, 253, 245), (16, 185, 129), f_callout, border=(167, 243, 208))
        # Callout 2: Diff view micro-edit
        draw_pill(draw, 840, 780, "✓ 双栏/单栏 Diff 查看 · 直接光标编辑微调 · ⌘S 即时落盘", (238, 242, 255), (79, 70, 229), f_callout, border=(199, 210, 254))
        # Callout 3: Unified commit & push
        draw_pill(draw, 1420, 780, "✓ 跨多仓一键统一提交与推送", (254, 243, 199), (180, 83, 9), f_callout, border=(253, 230, 138))

    # -------------------------------------------------------------
    # SCENE 3: ACTUAL BRANCH SWITCHER MODAL (7.0s - 10.5s, frames 210-315)
    # -------------------------------------------------------------
    elif f < 315:
        local_f = f - 210
        p = local_f / 105.0

        f_hdr = get_font(18, zh=True)
        draw_pill(draw, 100, 40, "FEATURE 02 · ⚡ 1-Click 多仓分支对齐与切换矩阵（实际程序界面）", (236, 253, 245), (16, 185, 129), f_hdr, pad_x=18, pad_y=8, border=(167, 243, 208))

        # Show the actual branch modal screenshot
        win_w = 1480
        win_img, ww, wh = render_window_mockup(SCREENS["branch"], win_w)
        im.paste(win_img, ((WIDTH - win_img.width) // 2, 100), win_img)

        # Highlights
        f_callout = get_font(13, zh=True)
        draw_pill(draw, 140, 880, "⚡ 统一输入分支名 · 模糊联想匹配 · 勾选仓库瞬时切换", (236, 253, 245), (16, 185, 129), f_callout, border=(167, 243, 208))
        draw_pill(draw, 800, 880, "🛡️ 有未提交改动的仓库自动安全跳过 · 绝不破坏工作区", (254, 243, 199), (180, 83, 9), f_callout, border=(253, 230, 138))
        draw_pill(draw, 1360, 880, "⭐ 记住分支组合 · 复杂需求一键还原", (238, 242, 255), (79, 70, 229), f_callout, border=(199, 210, 254))

    # -------------------------------------------------------------
    # SCENE 4: ACTUAL COMMIT FOREST & SWIMLANES (10.5s - 14.5s, frames 315-435)
    # -------------------------------------------------------------
    elif f < 435:
        local_f = f - 315
        p = local_f / 120.0

        f_hdr = get_font(18, zh=True)
        draw_pill(draw, 100, 40, "FEATURE 03 · 🌳 统一多仓提交森林与 Worktree 隔离合并（实际程序界面）", (254, 243, 199), (180, 83, 9), f_hdr, pad_x=18, pad_y=8, border=(253, 230, 138))

        # Show actual commit forest screenshot
        win_w = 1520
        win_img, ww, wh = render_window_mockup(SCREENS["forest"], win_w)
        im.paste(win_img, ((WIDTH - win_img.width) // 2, 105), win_img)

        f_callout = get_font(13, zh=True)
        draw_pill(draw, 140, 880, "🌳 多仓提交按时间统一拓扑编织 · 活跃彩色泳道", (254, 243, 199), (180, 83, 9), f_callout, border=(253, 230, 138))
        draw_pill(draw, 740, 880, "🛡️ Worktree 隔离合并 · 零触碰本地工作区", (236, 253, 245), (16, 185, 129), f_callout, border=(167, 243, 208))
        draw_pill(draw, 1260, 880, "🤖 AI 冲突消解 6 项机器强安全校验", (238, 242, 255), (79, 70, 229), f_callout, border=(199, 210, 254))

    # -------------------------------------------------------------
    # SCENE 5: FINALE & DOWNLOAD CALL-TO-ACTION (14.5s - 18.0s, frames 435-540)
    # -------------------------------------------------------------
    else:
        local_f = f - 435
        p = local_f / 105.0

        cx, cy = WIDTH // 2, HEIGHT // 2 - 20

        # Big App Icon
        if APP_ICON:
            isz = 200
            icon_res = APP_ICON.resize((isz, isz), Image.Resampling.LANCZOS)
            ishadow = Image.new("RGBA", (isz + 40, isz + 40), (0, 0, 0, 0))
            isdraw = ImageDraw.Draw(ishadow)
            isdraw.rounded_rectangle([20, 24, 20 + isz, 24 + isz], radius=44, fill=(0, 0, 0, 30))
            ishadow = ishadow.filter(ImageFilter.GaussianBlur(16))
            im.paste(ishadow, (cx - isz // 2 - 20, cy - isz // 2 - 90 - 20), ishadow)
            im.paste(icon_res, (cx - isz // 2, cy - isz // 2 - 90), icon_res)

        f_big = get_font(48, bold=True)
        t_text = "Nemorous v0.2.0"
        tb = f_big.getbbox(t_text)
        draw.text((cx - (tb[2] - tb[0]) // 2, cy + 50), t_text, fill=(17, 24, 39), font=f_big)

        f_sub = get_font(22, zh=True)
        s_text = "多仓如林 · 统一俯瞰 · 开源旗舰桌面工具"
        sb = f_sub.getbbox(s_text)
        draw.text((cx - (sb[2] - sb[0]) // 2, cy + 115), s_text, fill=(16, 185, 129), font=f_sub)

        # Platforms
        f_pill = get_font(14, zh=True)
        pl1 = "🍏 macOS (Apple Silicon / Intel)"
        pl2 = "🪟 Windows (x64 / ARM64)"
        pl3 = "🐧 Linux (deb / tar.gz)"

        pw_total = 290 + 250 + 220 + 24
        px = (WIDTH - pw_total) // 2
        py = cy + 165
        w, _ = draw_pill(draw, px, py, pl1, (255, 255, 255), (17, 24, 39), f_pill, border=(215, 220, 230))
        px += w + 12
        w, _ = draw_pill(draw, px, py, pl2, (255, 255, 255), (17, 24, 39), f_pill, border=(215, 220, 230))
        px += w + 12
        w, _ = draw_pill(draw, px, py, pl3, (255, 255, 255), (17, 24, 39), f_pill, border=(215, 220, 230))

        # CTA
        f_cta = get_font(20, bold=True)
        cta_text = "⭐ github.com/iwangjie/Nemorous"
        draw_pill(draw, cx - 215, cy + 230, cta_text, (79, 70, 229), (255, 255, 255), f_cta, pad_x=36, pad_y=14, radius=12)

    # Save
    out_path = f"{FRAME_DIR}/frame_{f:04d}.png"
    im.convert("RGB").save(out_path, "PNG", compress_level=1)

def main():
    if os.path.exists(FRAME_DIR):
        shutil.rmtree(FRAME_DIR)
    os.makedirs(FRAME_DIR, exist_ok=True)
    os.makedirs("assets", exist_ok=True)

    print(f"Rendering {TOTAL_FRAMES} frames with actual screenshots...")
    with ProcessPoolExecutor(max_workers=8) as executor:
        list(executor.map(render_frame, range(TOTAL_FRAMES)))

    print("Encoding MP4 video with ffmpeg...")
    mp4_cmd = [
        "ffmpeg", "-y",
        "-framerate", str(FPS),
        "-i", f"{FRAME_DIR}/frame_%04d.png",
        "-c:v", "libx264",
        "-pix_fmt", "yuv420p",
        "-crf", "18",
        "-preset", "fast",
        "assets/promo.mp4"
    ]
    subprocess.run(mp4_cmd, check=True)
    print("MP4 generated: assets/promo.mp4")

    print("Encoding optimized animated GIF...")
    gif_cmd = [
        "ffmpeg", "-y",
        "-ss", "0", "-t", "18",
        "-i", "assets/promo.mp4",
        "-vf", "fps=12,scale=800:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=bayer",
        "assets/preview.gif"
    ]
    subprocess.run(gif_cmd, check=True)
    print("GIF generated: assets/preview.gif")

    shutil.rmtree(FRAME_DIR)
    print("Flagship promo video generation complete!")

if __name__ == "__main__":
    main()
