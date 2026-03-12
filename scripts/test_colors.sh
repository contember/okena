#!/usr/bin/env bash
# Terminal color capability test script
# Run inside a terminal to see what it supports

printf "\n=== Terminal Color Capabilities Test ===\n"
printf "TERM=%s  COLORTERM=%s\n\n" "$TERM" "$COLORTERM"

# --- 1. Basic 8 ANSI colors (SGR 30-37) ---
printf "── 1. Basic ANSI Colors (30-37) ──\n"
for i in {0..7}; do
    printf "\e[%dm %-7s \e[0m" "$((30+i))" "Color$i"
done
printf "\n"
for i in {0..7}; do
    printf "\e[%dm %-7s \e[0m" "$((40+i))" "  bg$i  "
done
printf "\n\n"

# --- 2. Bright ANSI colors (SGR 90-97) ---
printf "── 2. Bright ANSI Colors (90-97) ──\n"
for i in {0..7}; do
    printf "\e[%dm %-7s \e[0m" "$((90+i))" "Bright$i"
done
printf "\n"
for i in {0..7}; do
    printf "\e[%dm %-7s \e[0m" "$((100+i))" " bg$i   "
done
printf "\n\n"

# --- 3. BOLD + basic colors (should look like bright in most terminals) ---
printf "── 3. BOLD + Basic Colors (should match bright row above) ──\n"
for i in {0..7}; do
    printf "\e[1;%dm %-7s \e[0m" "$((30+i))" "Bold+$i"
done
printf "\n"
printf "   ^ If these look identical to row 1 (not row 2), BOLD→bright is broken\n\n"

# --- 4. DIM attribute (SGR 2) ---
printf "── 4. DIM Attribute ──\n"
printf "   Normal:  \e[37mThe quick brown fox\e[0m\n"
printf "   DIM:     \e[2;37mThe quick brown fox\e[0m\n"
printf "   BOLD:    \e[1;37mThe quick brown fox\e[0m\n"
printf "   ^ DIM should be visibly darker than Normal\n\n"

# --- 5. 256-color palette (SGR 38;5;N) ---
printf "── 5. 256-Color Palette ──\n"
printf "   Standard (0-15):\n   "
for i in {0..15}; do
    printf "\e[48;5;%dm  \e[0m" "$i"
done
printf "\n   216 Color Cube (16-231):\n"
for row in 0 1 2 3 4 5; do
    printf "   "
    for col in {0..35}; do
        printf "\e[48;5;%dm \e[0m" "$((16 + row*36 + col))"
    done
    printf "\n"
done
printf "   Grayscale (232-255):\n   "
for i in {232..255}; do
    printf "\e[48;5;%dm  \e[0m" "$i"
done
printf "\n\n"

# --- 6. Truecolor / 24-bit (SGR 38;2;R;G;B) ---
printf "── 6. Truecolor (24-bit RGB) ──\n"
printf "   Red gradient:   "
for i in $(seq 0 8 255); do
    printf "\e[48;2;%d;0;0m \e[0m" "$i"
done
printf "\n   Green gradient: "
for i in $(seq 0 8 255); do
    printf "\e[48;2;0;%d;0m \e[0m" "$i"
done
printf "\n   Blue gradient:  "
for i in $(seq 0 8 255); do
    printf "\e[48;2;0;0;%dm \e[0m" "$i"
done
printf "\n   Rainbow:        "
for i in $(seq 0 4 255); do
    r=$(( i < 128 ? 255 - i*2 : 0 ))
    g=$(( i < 128 ? i*2 : 510 - i*2 ))
    b=$(( i < 128 ? 0 : i*2 - 255 ))
    # clamp
    r=$(( r < 0 ? 0 : r > 255 ? 255 : r ))
    g=$(( g < 0 ? 0 : g > 255 ? 255 : g ))
    b=$(( b < 0 ? 0 : b > 255 ? 255 : b ))
    printf "\e[48;2;%d;%d;%dm \e[0m" "$r" "$g" "$b"
done
printf "\n   ^ Gradients should be smooth, not banded\n\n"

# --- 7. Text attributes ---
printf "── 7. Text Attributes ──\n"
printf "   \e[0mNormal\e[0m  "
printf "\e[1mBold\e[0m  "
printf "\e[2mDim\e[0m  "
printf "\e[3mItalic\e[0m  "
printf "\e[4mUnderline\e[0m  "
printf "\e[4:3mUndercurl\e[0m  "
printf "\e[7mInverse\e[0m  "
printf "\e[9mStrikethrough\e[0m\n"
printf "   \e[1;3mBold+Italic\e[0m  "
printf "\e[2;3mDim+Italic\e[0m  "
printf "\e[1;4mBold+Underline\e[0m  "
printf "\e[2;4mDim+Underline\e[0m\n\n"

# --- 8. Colored underlines (SGR 58;2;R;G;B) ---
printf "── 8. Colored Underlines ──\n"
printf "   \e[4;58;2;255;0;0mRed underline\e[0m  "
printf "\e[4;58;2;0;255;0mGreen underline\e[0m  "
printf "\e[4;58;2;0;0;255mBlue underline\e[0m\n\n"

# --- 9. BOLD vs Bright side-by-side per color ---
printf "── 9. BOLD vs Bright Comparison ──\n"
names=(Black Red Green Yellow Blue Magenta Cyan White)
printf "   %-10s %-12s %-12s %-5s\n" "Color" "BOLD+basic" "Bright" "Match?"
for i in {0..7}; do
    bold=$(printf "\e[1;%dm████\e[0m" "$((30+i))")
    bright=$(printf "\e[%dm████\e[0m" "$((90+i))")
    printf "   %-10s %s      %s\n" "${names[$i]}" "$bold" "$bright"
done
printf "   ^ In a correct terminal, BOLD+basic and Bright should look the same\n\n"

# --- 10. DIM with truecolor ---
printf "── 10. DIM with Truecolor ──\n"
printf "   Normal:  \e[38;2;255;100;50mRGB(255,100,50) sample text\e[0m\n"
printf "   DIM:     \e[2;38;2;255;100;50mRGB(255,100,50) sample text\e[0m\n"
printf "   ^ DIM should visibly reduce brightness even with truecolor\n\n"

printf "=== End of Test ===\n"
