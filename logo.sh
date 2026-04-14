#!/usr/bin/env bash
# ============================================================
#  Qunix ASCII Logo — pixel-by-pixel animated reveal
#  Requires: Python3 (pre-installed on most Linux/macOS)
# ============================================================

python3 - "$@" << 'PYTHON_SCRIPT'
import sys, os, time, signal, random

# ── Terminal helpers ────────────────────────────────────────
def write(s):
    sys.stdout.write(s)
    sys.stdout.flush()

def move(row, col):
    write(f"\033[{row};{col}H")

def hide_cursor():
    write("\033[?25l")

def show_cursor():
    write("\033[?25h")

def clear_screen():
    write("\033[2J\033[H")

def reset_color():
    write("\033[0m")

def rgb(r, g, b, bg=False):
    layer = 48 if bg else 38
    return f"\033[{layer};2;{r};{g};{b}m"

# Shrink terminal font via xterm OSC + resize to wide/tall
def shrink_font():
    # OSC sequence for xterm font (size=4 = very small)
    write("\033]50;xft:Monospace:size=4\007")
    # Resize terminal window: 55 rows x 210 cols → makes each char tiny on screen
    write("\033[8;55;210t")
    time.sleep(0.15)

def restore_font():
    write("\033]50;xft:Monospace:size=11\007")
    write("\033[8;24;80t")
    time.sleep(0.05)

# ── Color palette  (matching Qunix logo blues/cyans) ────────
BLACK   = (0,   0,   0  )
DBLUE1  = (0,   15,  50 )   # deepest shadow
DBLUE2  = (0,   40,  110)   # dark feather core
MBLUE1  = (0,   80,  180)   # mid feather
MBLUE2  = (0,  130,  230)   # bright feather
CYAN1   = (0,  185,  255)   # electric cyan
CYAN2   = (0,  220,  255)   # near-white cyan
WHITE_C = (160, 235, 255)   # hottest highlight

def c(col, bg=False):
    return rgb(*col, bg=bg)

# ── The logo: each row is a list of (char, fg_color, bold) ─
# We build TWO sections side by side:
#   LEFT : phoenix/eagle bird  (cols 1-38)
#   RIGHT: "Qunix" block text  (cols 40-100)

BOLD   = "\033[1m"
RESET  = "\033[0m"

# ─────────────────────────────────────────────────────────────
# BIRD ART  (38 cols wide, 28 rows)
# Uses unicode block/braille chars for high density look
# Color per-character for gradient effect
# ─────────────────────────────────────────────────────────────

B = DBLUE1
D = DBLUE2
M = MBLUE1
L = MBLUE2
C = CYAN1
E = CYAN2
W = WHITE_C
_ = None  # transparent / space

# Each cell: (char, color) — None = space
# 38 wide × 28 tall
BIRD = [

]

# Color map for bird: gradient from dark core → electric tips
# We look at the character and assign color by its density
def char_to_color(ch, col_idx, row_idx, total_rows, total_cols):
    if ch == ' ':
        return None
    # Distance from center of bird (approx col 19, row 13)
    cx, cy = 19, 13
    dist = ((col_idx - cx)**2 + (row_idx - cy)**2) ** 0.5
    max_dist = 20.0
    t = min(dist / max_dist, 1.0)  # 0=center(dark), 1=edge(bright)

    # Block char density → brightness
    dense = {'█': 0.0, '▓': 0.1, '▐': 0.1, '▌': 0.1,
             '▄': 0.2, '▀': 0.2, '▊': 0.2, '▋': 0.3,
             '▇': 0.1, '▆': 0.2, '▅': 0.3, '▃': 0.4,
             '◈': -0.3,  # eye — brighter
             '▖': 0.4, '▗': 0.4, '▘': 0.4, '▝': 0.4}
    d_offset = dense.get(ch, 0.35)
    t2 = max(0.0, min(1.0, t + d_offset))

    # Interpolate through palette
    palette = [DBLUE1, DBLUE2, MBLUE1, MBLUE2, CYAN1, CYAN2, WHITE_C]
    idx_f = t2 * (len(palette) - 1)
    i = int(idx_f)
    frac = idx_f - i
    if i >= len(palette) - 1:
        return palette[-1]
    r1, g1, b1 = palette[i]
    r2, g2, b2 = palette[i+1]
    return (int(r1 + (r2-r1)*frac), int(g1 + (g2-g1)*frac), int(b1 + (b2-b1)*frac))

# ─────────────────────────────────────────────────────────────
# QUNIX TEXT  (block letter art, 55 wide × 10 tall)
# Each letter drawn with █ blocks, colored electric cyan
# ─────────────────────────────────────────────────────────────
TEXT_ROWS = [
  "  ██████  ██    ██ ███   ██ ██ ██   ██",
  " ██    ██ ██    ██ ████  ██ ██  ██ ██ ",
  " ██    ██ ██    ██ ██ ██ ██ ██   ███  ",
  " ██ ▄▄ ██ ██    ██ ██  ████ ██  ██ ██ ",
  "  ██████  ████████ ██  ████ ██ ██   ██",
  " ",
]

TAGLINE = "QunixOS"

# ─────────────────────────────────────────────────────────────
# Build pixel canvas
# Canvas: 55 rows × 100 cols
# Bird at rows 2..29, cols 1..44
# Text at rows 9..20, cols 46..100
# ─────────────────────────────────────────────────────────────
CANVAS_ROWS = 34
CANVAS_COLS = 100

# canvas[row][col] = (char, (r,g,b)) or None
canvas = [[None]*CANVAS_COLS for _ in range(CANVAS_ROWS)]

# Place bird
BIRD_START_ROW = 2
BIRD_START_COL = 1
for ri, line in enumerate(BIRD):
    for ci, ch in enumerate(line):
        if ch != ' ':
            row = BIRD_START_ROW + ri
            col = BIRD_START_COL + ci
            if 0 <= row < CANVAS_ROWS and 0 <= col < CANVAS_COLS:
                color = char_to_color(ch, ci, ri, len(BIRD), len(line))
                canvas[row][col] = (ch, color)

# Place text (right side)
TEXT_START_ROW = 10
TEXT_START_COL = 44
for ri, line in enumerate(TEXT_ROWS):
    for ci, ch in enumerate(line):
        if ch != ' ':
            row = TEXT_START_ROW + ri
            col = TEXT_START_COL + ci
            if 0 <= row < CANVAS_ROWS and 0 <= col < CANVAS_COLS:
                # Text gradient: left=mid-blue, right=electric-cyan
                t = ci / max(len(line), 1)
                r1,g1,b1 = MBLUE2
                r2,g2,b2 = CYAN2
                col_rgb = (int(r1+(r2-r1)*t), int(g1+(g2-g1)*t), int(b1+(b2-b1)*t))
                canvas[row][col] = (ch, col_rgb)

# Place tagline
TAG_ROW = 18
TAG_COL = 46
for ci, ch in enumerate(TAGLINE):
    col = TAG_COL + ci
    if ch != ' ' and 0 <= TAG_ROW < CANVAS_ROWS and 0 <= col < CANVAS_COLS:
        t = ci / len(TAGLINE)
        r1,g1,b1 = CYAN1
        r2,g2,b2 = WHITE_C
        col_rgb = (int(r1+(r2-r1)*t), int(g1+(g2-g1)*t), int(b1+(b2-b1)*t))
        canvas[TAG_ROW][col] = (ch, col_rgb)

# Divider line
DIV_ROW = 17
for ci in range(TAG_COL, TAG_COL + len(TAGLINE) + 2):
    if 0 <= ci < CANVAS_COLS:
        canvas[DIV_ROW][ci] = ('─', MBLUE2)
DIV_ROW2 = 19
for ci in range(TAG_COL, TAG_COL + len(TAGLINE) + 2):
    if 0 <= ci < CANVAS_COLS:
        canvas[DIV_ROW2][ci] = ('─', MBLUE2)

# Bottom glow line under bird
GLOW_ROW = BIRD_START_ROW + len(BIRD)
for ci in range(BIRD_START_COL, BIRD_START_COL + 42):
    if 0 <= GLOW_ROW < CANVAS_ROWS and 0 <= ci < CANVAS_COLS:
        if canvas[GLOW_ROW][ci] is None:
            t = abs(ci - (BIRD_START_COL + 21)) / 21.0
            alpha = max(0.0, 1.0 - t)
            glow_g = int(60 * alpha)
            glow_b = int(120 * alpha)
            if glow_g > 5:
                canvas[GLOW_ROW][ci] = ('░', (0, glow_g, glow_b))

# ─────────────────────────────────────────────────────────────
# Collect all non-None pixels for animation ordering
# ─────────────────────────────────────────────────────────────
pixels = []
for ri in range(CANVAS_ROWS):
    for ci in range(CANVAS_COLS):
        cell = canvas[ri][ci]
        if cell is not None:
            pixels.append((ri, ci, cell[0], cell[1]))

# Sort: column sweep left-to-right, top-to-bottom (scan line)
pixels.sort(key=lambda p: (p[1], p[0]))

# ─────────────────────────────────────────────────────────────
# Signal handler
# ─────────────────────────────────────────────────────────────
def cleanup(sig=None, frame=None):
    show_cursor()
    restore_font()
    clear_screen()
    reset_color()
    print("\033[38;2;0;210;255mTerminal restored. Goodbye.\033[0m")
    sys.exit(0)

signal.signal(signal.SIGINT,  cleanup)
signal.signal(signal.SIGTERM, cleanup)

# ─────────────────────────────────────────────────────────────
# MAIN — Draw!
# ─────────────────────────────────────────────────────────────
shrink_font()
time.sleep(0.2)
clear_screen()
hide_cursor()

# Black background fill
write(rgb(*BLACK, bg=True))
for row in range(CANVAS_ROWS + 4):
    move(row+1, 1)
    write(' ' * CANVAS_COLS)
reset_color()

# ── Pixel-by-pixel draw with scan-line sweep ────────────────
# Group into columns, draw each column top→bottom rapidly
# then tiny pause between columns → "beam scan" effect

SCREEN_OFFSET_ROW = 2   # start drawing at screen row 2
SCREEN_OFFSET_COL = 3   # start drawing at screen col 3

prev_col = -1
col_pixels = {}

# Group by canvas column
for (ri, ci, ch, color) in pixels:
    col_pixels.setdefault(ci, []).append((ri, ch, color))

# Draw column by column (left→right) with fast per-pixel output
sorted_cols = sorted(col_pixels.keys())

for ci in sorted_cols:
    cells = sorted(col_pixels[ci], key=lambda x: x[0])  # top→bottom
    for (ri, ch, color) in cells:
        screen_row = SCREEN_OFFSET_ROW + ri
        screen_col = SCREEN_OFFSET_COL + ci
        move(screen_row, screen_col)
        # Glow effect: bold for bright pixels
        r,g,b = color
        bright = (r + g + b) / 3
        if bright > 150:
            write(BOLD + rgb(r,g,b) + ch + RESET)
        else:
            write(rgb(r,g,b) + ch + RESET)
    # Tiny pause between columns → scan-line animation
    sys.stdout.flush()
    time.sleep(0.012)

# ── Final glow pulse on the text ───────────────────────────
time.sleep(0.3)
# Flash the Qunix text brighter
for ri, line in enumerate(TEXT_ROWS):
    for ci, ch in enumerate(line):
        if ch != ' ':
            row = TEXT_START_ROW + ri
            col  = TEXT_START_COL + ci
            screen_row = SCREEN_OFFSET_ROW + row
            screen_col = SCREEN_OFFSET_COL + col
            move(screen_row, screen_col)
            write(BOLD + rgb(*WHITE_C) + ch + RESET)
sys.stdout.flush()
time.sleep(0.15)

# Settle back to normal colors
for ri, line in enumerate(TEXT_ROWS):
    for ci, ch in enumerate(line):
        if ch != ' ':
            row = TEXT_START_ROW + ri
            col  = TEXT_START_COL + ci
            screen_row = SCREEN_OFFSET_ROW + row
            screen_col = SCREEN_OFFSET_COL + col
            move(screen_row, screen_col)
            t = ci / max(len(line), 1)
            r1,g1,b1 = MBLUE2
            r2,g2,b2 = CYAN2
            cr = (int(r1+(r2-r1)*t), int(g1+(g2-g1)*t), int(b1+(b2-b1)*t))
            write(BOLD + rgb(*cr) + ch + RESET)
sys.stdout.flush()

# ── Status line at bottom ───────────────────────────────────
bottom = SCREEN_OFFSET_ROW + CANVAS_ROWS + 1
move(bottom, SCREEN_OFFSET_COL)
write(rgb(*MBLUE1) + "  Press " + BOLD + rgb(*CYAN2) + "Ctrl+C" +
      RESET + rgb(*MBLUE1) + " to restore terminal and exit." + RESET)
sys.stdout.flush()

# ── Wait for Ctrl+C ─────────────────────────────────────────
while True:
    time.sleep(1)

PYTHON_SCRIPT