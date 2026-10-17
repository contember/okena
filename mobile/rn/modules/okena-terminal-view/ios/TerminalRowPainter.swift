import CoreGraphics
import CoreText
import Foundation
import OkenaRender
import UIKit

// MARK: - Fonts

/// The terminal fonts at one size, plus the cell metrics derived from them.
///
/// Mirrors the Skia renderer (`TerminalView.tsx`, `terminalGlyphs.ts`,
/// `terminalFonts.ts`):
///   - JetBrains Mono in all four variants, from the app bundle (UIAppFonts);
///   - cell width = advance of "M", cell height = (ascent + descent) ×
///     `lineHeightFactor`, glyph box centered vertically in the cell;
///   - per-codepoint fallback: primary font → primary-font alias (`⎿` → `└`)
///     → bundled Noto Sans Symbols 2 (not for emoji-presentation codepoints)
///     → the system fallback (Core Text's per-character match).
///
/// One difference: the cell height is rounded to whole device pixels, so every
/// row starts on a pixel boundary and a cached row bitmap stays sharp wherever
/// the row moves. That changes the height by at most half a pixel.
final class TerminalFonts {
  static let lineHeightFactor: CGFloat = 1.2

  let size: CGFloat
  let scale: CGFloat
  /// Indexed by `styleIndex(bold:italic:)`.
  let primary: [CTFont]
  let symbols: CTFont?
  let emoji: CTFont
  let cellWidth: CGFloat
  let cellHeight: CGFloat
  /// Baseline offset from the row top.
  let baseline: CGFloat
  /// Top and bottom of the full block `█` glyph in row coordinates. The
  /// painter fills `█` as a rect over this span: adjacent anti-aliased glyph
  /// edges at fractional cell positions left faint seams in progress bars.
  let fullBlockTop: CGFloat
  let fullBlockBottom: CGFloat

  private static let symbolsDescriptor: CTFontDescriptor? = {
    guard let url = Bundle.main.url(forResource: "NotoSansSymbols2-Subset", withExtension: "ttf"),
      let descriptors = CTFontManagerCreateFontDescriptorsFromURL(url as CFURL) as? [CTFontDescriptor]
    else { return nil }
    return descriptors.first
  }()

  private static let primaryNames = [
    "JetBrainsMono-Regular", "JetBrainsMono-Bold", "JetBrainsMono-Italic", "JetBrainsMono-BoldItalic",
  ]

  static func styleIndex(bold: Bool, italic: Bool) -> Int {
    (bold ? 1 : 0) | (italic ? 2 : 0)
  }

  init(size: CGFloat, scale: CGFloat) {
    self.size = size
    self.scale = max(scale, 1)
    // Noto Sans Symbols 2 first in the cascade list, then the system default
    // cascade: graphemes drawn as a CTLine pick it up automatically.
    var attributes: [CFString: Any] = [:]
    if let symbols = Self.symbolsDescriptor {
      attributes[kCTFontCascadeListAttribute] = [symbols]
    }
    let cascade = CTFontDescriptorCreateWithAttributes(attributes as CFDictionary)
    primary = Self.primaryNames.enumerated().map { index, name in
      let base = CTFontCreateWithName(name as CFString, size, nil)
      // A missing variant falls back to the system monospace font (never
      // expected: all four are bundled).
      let resolved: CTFont
      if (CTFontCopyPostScriptName(base) as String) == name {
        resolved = base
      } else {
        let weight: UIFont.Weight = (index & 1) != 0 ? .bold : .regular
        resolved = UIFont.monospacedSystemFont(ofSize: size, weight: weight) as CTFont
      }
      return CTFontCreateCopyWithAttributes(resolved, size, nil, cascade)
    }
    symbols = Self.symbolsDescriptor.map { CTFontCreateWithFontDescriptor($0, size, nil) }
    emoji = CTFontCreateWithName("AppleColorEmoji" as CFString, size, nil)

    let regular = primary[0]
    var ch: UniChar = 0x4D  // "M"
    var glyph: CGGlyph = 0
    CTFontGetGlyphsForCharacters(regular, &ch, &glyph, 1)
    var advance = CGSize.zero
    CTFontGetAdvancesForGlyphs(regular, .horizontal, &glyph, &advance, 1)
    let ascent = CTFontGetAscent(regular)
    let textHeight = ascent + CTFontGetDescent(regular)
    let rawHeight = textHeight * Self.lineHeightFactor
    let height = max((rawHeight * self.scale).rounded() / self.scale, 1 / self.scale)
    cellWidth = advance.width
    cellHeight = height
    let baseline = (height - textHeight) / 2 + ascent
    self.baseline = baseline
    var block: UniChar = 0x2588
    var blockGlyph: CGGlyph = 0
    var box = CGRect.zero
    if CTFontGetGlyphsForCharacters(regular, &block, &blockGlyph, 1) {
      box = CTFontGetBoundingRectsForGlyphs(regular, .horizontal, &blockGlyph, nil, 1)
    }
    if box.height > 0 {
      fullBlockTop = max(baseline - box.maxY, 0)
      fullBlockBottom = min(baseline - box.minY, height)
    } else {
      fullBlockTop = 0
      fullBlockBottom = height
    }
  }

  // MARK: Glyph resolution

  enum Glyph {
    /// Draw `glyph` from the primary font of the cell's style.
    case primary(CGGlyph)
    /// Draw `glyph` from `font`, fitted into the cell(s).
    case fallback(CTFont, CGGlyph, advance: CGFloat)
  }

  private var cache: [UInt64: Glyph] = [:]

  /// Codepoints that render as colour emoji by default (wide symbols in the
  /// emoji blocks). Mirrors `isEmojiPresentation` in terminalGlyphs.ts, with
  /// "wide" taken from the frame's WIDE flag.
  static func isEmojiPresentation(_ cp: UInt32, wide: Bool) -> Bool {
    guard wide else { return false }
    return (cp >= 0x231A && cp <= 0x2B55) || (cp >= 0x1F000 && cp < 0x1FB00)
  }

  /// Stand-ins drawn with the primary font (`GLYPH_ALIASES`).
  private static func alias(_ cp: UInt32) -> UInt32? {
    cp == 0x23BF ? 0x2514 : nil  // ⎿ → └
  }

  func resolve(_ cp: UInt32, style: Int, wide: Bool) -> Glyph {
    let key = UInt64(cp) | (UInt64(style) << 32) | (wide ? (1 << 40) : 0)
    if let hit = cache[key] { return hit }
    let result = compute(cp, style: style, wide: wide)
    cache[key] = result
    return result
  }

  private func compute(_ cp: UInt32, style: Int, wide: Bool) -> Glyph {
    let font = primary[style]
    let g = Self.glyph(font, cp)
    if g != 0 { return .primary(g) }
    if let alias = Self.alias(cp) {
      let ag = Self.glyph(font, alias)
      if ag != 0 { return .primary(ag) }
    }
    let emojiPresentation = Self.isEmojiPresentation(cp, wide: wide)
    if !emojiPresentation, let symbols {
      let sg = Self.glyph(symbols, cp)
      if sg != 0 { return fallback(symbols, sg) }
    }
    if emojiPresentation {
      let eg = Self.glyph(emoji, cp)
      if eg != 0 { return fallback(emoji, eg) }
    }
    // System per-character match.
    if let scalar = Unicode.Scalar(cp) {
      let string = String(Character(scalar)) as CFString
      let matched = CTFontCreateForString(primary[0], string, CFRange(location: 0, length: CFStringGetLength(string)))
      let mg = Self.glyph(matched, cp)
      if mg != 0 { return fallback(matched, mg) }
    }
    // Nothing has it: the primary font draws its missing-glyph box.
    return .primary(0)
  }

  private func fallback(_ font: CTFont, _ glyph: CGGlyph) -> Glyph {
    var g = glyph
    var advance = CGSize.zero
    CTFontGetAdvancesForGlyphs(font, .horizontal, &g, &advance, 1)
    return .fallback(font, glyph, advance: advance.width)
  }

  static func glyph(_ font: CTFont, _ cp: UInt32) -> CGGlyph {
    var chars: [UniChar] = []
    if cp > 0xFFFF {
      let v = cp - 0x10000
      chars = [UniChar(0xD800 + (v >> 10)), UniChar(0xDC00 + (v & 0x3FF))]
    } else {
      chars = [UniChar(cp)]
    }
    var glyphs: [CGGlyph] = [0, 0]
    guard CTFontGetGlyphsForCharacters(font, chars, &glyphs, chars.count) else { return 0 }
    return glyphs[0]
  }
}

// MARK: - Row painting

/// Paints one terminal row into a bitmap with Core Text.
///
/// Same passes as `paintTerminal` in TerminalView.tsx, per row: cell
/// backgrounds, glyph runs batched by colour and style, fallback glyphs
/// fitted into their cell(s), then underline / strikethrough. The selection
/// tint goes over the cell backgrounds and under the text, as in Skia's pass 1.
/// The cursor and the scroll indicator are overlays and never baked in.
final class TerminalRowPainter {
  let fonts: TerminalFonts
  let widthPt: CGFloat
  let pixelWidth: Int
  let pixelHeight: Int
  private let context: CGContext
  private var glyphs: [CGGlyph] = []
  private var positions: [CGPoint] = []

  init?(fonts: TerminalFonts, width: CGFloat) {
    self.fonts = fonts
    widthPt = width
    pixelWidth = max(Int((width * fonts.scale).rounded(.up)), 1)
    pixelHeight = max(Int((fonts.cellHeight * fonts.scale).rounded()), 1)
    guard
      let ctx = CGContext(
        data: nil, width: pixelWidth, height: pixelHeight, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
    else { return nil }
    context = ctx
    // Top-left origin in points, like UIKit; glyphs upright via the flipped
    // text matrix. CTFontDrawGlyphs positions are in text space, so their y is
    // negated (-baseline); CTLine's textPosition is in user space.
    ctx.translateBy(x: 0, y: CGFloat(pixelHeight))
    ctx.scaleBy(x: fonts.scale, y: -fonts.scale)
    ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
    ctx.setAllowsFontSubpixelPositioning(true)
    ctx.setShouldSubpixelPositionFonts(true)
    ctx.setAllowsFontSmoothing(false)
    glyphs.reserveCapacity(512)
    positions.reserveCapacity(512)
  }

  @inline(__always)
  private func setFill(_ argb: UInt32) {
    context.setFillColor(
      red: CGFloat((argb >> 16) & 0xff) / 255, green: CGFloat((argb >> 8) & 0xff) / 255,
      blue: CGFloat(argb & 0xff) / 255, alpha: CGFloat(argb >> 24) / 255)
  }

  /// Fill pixel columns [x0, x1) of every pixel row with an opaque colour.
  /// With noneSkipFirst + byteOrder32Little a pixel read as a little-endian
  /// UInt32 is 0xXXRRGGBB, the same layout as our ARGB values.
  private func fillPixels(from x0: Int, to x1: Int, color: UInt32) {
    guard x1 > x0, let data = context.data else { return }
    var pattern = color
    let stride = context.bytesPerRow
    for y in 0..<pixelHeight {
      memset_pattern4(data + y * stride + x0 * 4, &pattern, (x1 - x0) * 4)
    }
  }

  /// `top` (ARGB, any alpha) composited over an opaque `bottom`.
  static func over(_ top: UInt32, _ bottom: UInt32) -> UInt32 {
    let a = top >> 24
    if a == 0xff { return top }
    if a == 0 { return bottom }
    func mix(_ shift: UInt32) -> UInt32 {
      let t = (top >> shift) & 0xff
      let b = (bottom >> shift) & 0xff
      return ((t * a + b * (255 - a) + 127) / 255) << shift
    }
    return 0xff00_0000 | mix(16) | mix(8) | mix(0)
  }

  /// Dim halves the alpha (rounded), as in the Skia painter.
  @inline(__always)
  static func effectiveFg(_ fg: UInt32, flags: UInt16) -> UInt32 {
    guard flags & UInt16(OKENA_RV_CELL_DIM) != 0 else { return fg }
    let a = UInt32((Double(fg >> 24) * 0.5).rounded())
    return (a << 24) | (fg & 0x00ff_ffff)
  }

  private static let styleMask = UInt16(
    OKENA_RV_CELL_BOLD | OKENA_RV_CELL_ITALIC | OKENA_RV_CELL_UNDERLINE | OKENA_RV_CELL_STRIKE
      | OKENA_RV_CELL_DIM)
  private static let spacerMask = UInt16(OKENA_RV_CELL_WIDE_SPACER | OKENA_RV_CELL_LEADING_WIDE_SPACER)

  /// Draw row `row` of `frame` and return the bitmap. `selection` is the
  /// row's selected column span, tinted with `selectionColor`.
  func paint(
    frame: UnsafePointer<OkenaRvFrame>, row: UInt16, background: UInt32,
    selection: ClosedRange<Int>? = nil, selectionColor: UInt32 = 0
  ) -> CGImage? {
    let f = frame.pointee
    let cols = Int(f.cols)
    let cells = okena_rv_frame_row_cells(frame, row)!
    let cw = fonts.cellWidth
    let ch = fonts.cellHeight
    let ctx = context

    // Pass 1: backgrounds, written straight into the bitmap (CG rect fills
    // were the biggest cost in a profile): the view background first (the
    // strip right of the last column), then every cell's own background in
    // same-colour runs, on whole device pixels as with antialiasing off.
    let opaqueBackground = background | 0xff00_0000
    let pxPerCell = cw * fonts.scale
    let gridEnd = min(Int((CGFloat(cols) * pxPerCell).rounded()), pixelWidth)
    fillPixels(from: gridEnd, to: pixelWidth, color: opaqueBackground)
    var c = 0
    while c < cols {
      let bg = cells[c].bg
      var e = c + 1
      while e < cols && cells[e].bg == bg { e += 1 }
      let x0 = Int((CGFloat(c) * pxPerCell).rounded())
      let x1 = min(Int((CGFloat(e) * pxPerCell).rounded()), pixelWidth)
      fillPixels(from: x0, to: x1, color: Self.over(bg, opaqueBackground))
      c = e
    }
    if let selection {
      for c in selection where c < cols {
        let x0 = Int((CGFloat(c) * pxPerCell).rounded())
        let x1 = min(Int((CGFloat(c + 1) * pxPerCell).rounded()), pixelWidth)
        fillPixels(
          from: x0, to: x1,
          color: Self.over(selectionColor, Self.over(cells[c].bg, opaqueBackground)))
      }
    }

    // Pass 2: text.
    let baseline = fonts.baseline
    var runFont: CTFont?
    var runFg: UInt32 = 0
    glyphs.removeAll(keepingCapacity: true)
    positions.removeAll(keepingCapacity: true)

    func flush() {
      if let font = runFont, !glyphs.isEmpty {
        setFill(runFg)
        CTFontDrawGlyphs(font, glyphs, positions, glyphs.count, ctx)
      }
      glyphs.removeAll(keepingCapacity: true)
      positions.removeAll(keepingCapacity: true)
      runFont = nil
    }

    var col = 0
    while col < cols {
      let cell = cells[col]
      let flags = cell.flags
      let cp = cell.codepoint
      if flags & Self.spacerMask != 0 || flags & UInt16(OKENA_RV_CELL_HIDDEN) != 0
        || (cell.extra == 0 && (cp == 0x20 || cp == 0))
      {
        col += 1
        continue
      }
      let wide = flags & UInt16(OKENA_RV_CELL_WIDE) != 0 && col + 1 < cols
      let span = wide ? 2 : 1
      let x = CGFloat(col) * cw
      let fg = Self.effectiveFg(cell.fg, flags: flags)
      let style = TerminalFonts.styleIndex(
        bold: flags & UInt16(OKENA_RV_CELL_BOLD) != 0, italic: flags & UInt16(OKENA_RV_CELL_ITALIC) != 0)

      if cp == 0x2588 && cell.extra == 0 {
        // Full block: a rect on whole device pixels, so a bar has no seams.
        flush()
        setFill(fg)
        let px0 = (CGFloat(col) * cw * fonts.scale).rounded() / fonts.scale
        let px1 = (CGFloat(col + span) * cw * fonts.scale).rounded() / fonts.scale
        let top = (fonts.fullBlockTop * fonts.scale).rounded() / fonts.scale
        let bottom = (fonts.fullBlockBottom * fonts.scale).rounded() / fonts.scale
        ctx.fill(CGRect(x: px0, y: top, width: px1 - px0, height: bottom - top))
      } else if cell.extra != 0 {
        flush()
        drawGrapheme(frame: frame, cell: cell, style: style, x: x, width: CGFloat(span) * cw, fg: fg)
      } else {
        switch fonts.resolve(cp, style: style, wide: wide) {
        case .primary(let g):
          let font = fonts.primary[style]
          if runFont !== font || runFg != fg { flush() }
          runFont = font
          runFg = fg
          glyphs.append(g)
          positions.append(CGPoint(x: x, y: -baseline))
        case .fallback(let font, let g, let advance):
          flush()
          setFill(fg)
          drawFitted(font: font, glyph: g, advance: advance, x: x, width: CGFloat(span) * cw)
        }
      }
      col += span
    }
    flush()

    // Pass 3: underline / strikethrough, merged over same-colour cells. Blank
    // cells keep their decoration (as in other terminals and on Android); the
    // Skia painter skipped them.
    let thickness = max(1, fonts.size / 14)
    var lineStart = -1
    var lineEnd = 0
    var lineFg: UInt32 = 0
    var lineFlags: UInt16 = 0
    let decoMask = UInt16(
      OKENA_RV_CELL_UNDERLINE | OKENA_RV_CELL_DOUBLE_UNDERLINE | OKENA_RV_CELL_STRIKE)

    func flushLine() {
      guard lineStart >= 0 else { return }
      setFill(lineFg)
      let x0 = CGFloat(lineStart) * cw
      let w = CGFloat(lineEnd - lineStart) * cw
      if lineFlags & UInt16(OKENA_RV_CELL_UNDERLINE) != 0 {
        ctx.fill(CGRect(x: x0, y: ch - 1 - thickness / 2, width: w, height: thickness))
        if lineFlags & UInt16(OKENA_RV_CELL_DOUBLE_UNDERLINE) != 0 {
          ctx.fill(CGRect(x: x0, y: ch - 1 - thickness * 2.5, width: w, height: thickness))
        }
      }
      if lineFlags & UInt16(OKENA_RV_CELL_STRIKE) != 0 {
        ctx.fill(CGRect(x: x0, y: ch / 2 - thickness / 2, width: w, height: thickness))
      }
      lineStart = -1
    }

    col = 0
    while col < cols {
      let cell = cells[col]
      let flags = cell.flags
      let span = flags & UInt16(OKENA_RV_CELL_WIDE) != 0 && col + 1 < cols ? 2 : 1
      let deco = flags & decoMask
      if flags & Self.spacerMask != 0 || deco == 0 {
        flushLine()
        col += 1
        continue
      }
      let fg = Self.effectiveFg(cell.fg, flags: flags)
      if lineStart >= 0 && (fg != lineFg || deco != lineFlags) { flushLine() }
      if lineStart < 0 {
        lineStart = col
        lineFg = fg
        lineFlags = deco
      }
      lineEnd = col + span
      col += span
    }
    flushLine()

    return ctx.makeImage()
  }

  /// A fallback glyph fitted into `width`: shrunk when wider (scaled around
  /// the row's vertical center), centered horizontally (`drawFallbackGlyph`).
  private func drawFitted(font: CTFont, glyph: CGGlyph, advance: CGFloat, x: CGFloat, width: CGFloat) {
    var g = glyph
    let scale = advance > width && advance > 0 ? width / advance : 1
    let dx = (width - advance * scale) / 2
    if scale == 1 {
      var p = CGPoint(x: x + dx, y: -fonts.baseline)
      CTFontDrawGlyphs(font, &g, &p, 1, context)
      return
    }
    let ch = fonts.cellHeight
    context.saveGState()
    context.translateBy(x: x + dx, y: ch / 2)
    context.scaleBy(x: scale, y: scale)
    var p = CGPoint(x: 0, y: ch / 2 - fonts.baseline)
    CTFontDrawGlyphs(font, &g, &p, 1, context)
    context.restoreGState()
  }

  /// A full grapheme (base + combining marks, ZWJ / flag sequences) laid out
  /// by Core Text and fitted into its cell(s).
  private func drawGrapheme(
    frame: UnsafePointer<OkenaRvFrame>, cell: OkenaRvCell, style: Int, x: CGFloat, width: CGFloat,
    fg: UInt32
  ) {
    var len: UInt32 = 0
    guard let ptr = okena_rv_frame_extra(frame, cell.extra, &len), len > 0 else { return }
    let text = ptr.withMemoryRebound(to: UInt8.self, capacity: Int(len)) {
      String(decoding: UnsafeBufferPointer(start: $0, count: Int(len)), as: UTF8.self)
    }
    let emoji = text.unicodeScalars.contains {
      $0.value == 0xFE0F || $0.value == 0x200D || ($0.value >= 0x1F1E6 && $0.value <= 0x1F1FF)
        || TerminalFonts.isEmojiPresentation($0.value, wide: true) && width > fonts.cellWidth * 1.5
    }
    let font = emoji ? fonts.emoji : fonts.primary[style]
    let attributed = NSAttributedString(
      string: text,
      attributes: [
        NSAttributedString.Key(kCTFontAttributeName as String): font,
        NSAttributedString.Key(kCTForegroundColorFromContextAttributeName as String): true,
      ])
    let line = CTLineCreateWithAttributedString(attributed)
    let advance = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
    let scale = advance > width && advance > 0 ? width / advance : 1
    let dx = (width - advance * scale) / 2
    setFill(fg)
    let ch = fonts.cellHeight
    context.saveGState()
    if scale == 1 {
      context.textPosition = CGPoint(x: x + dx, y: fonts.baseline)
    } else {
      context.translateBy(x: x + dx, y: ch / 2)
      context.scaleBy(x: scale, y: scale)
      context.textPosition = CGPoint(x: 0, y: fonts.baseline - ch / 2)
    }
    CTLineDraw(line, context)
    context.restoreGState()
    // The text position is not part of the graphics state, and
    // CTFontDrawGlyphs adds it to every glyph position.
    context.textPosition = .zero
  }

  /// The row's text, for VoiceOver.
  static func text(frame: UnsafePointer<OkenaRvFrame>, row: UInt16) -> String {
    let cols = Int(frame.pointee.cols)
    guard let cells = okena_rv_frame_row_cells(frame, row) else { return "" }
    var out = String.UnicodeScalarView()
    for c in 0..<cols {
      let cell = cells[c]
      if cell.flags & spacerMask != 0 { continue }
      if cell.extra != 0 {
        var len: UInt32 = 0
        if let ptr = okena_rv_frame_extra(frame, cell.extra, &len) {
          let s = ptr.withMemoryRebound(to: UInt8.self, capacity: Int(len)) {
            String(decoding: UnsafeBufferPointer(start: $0, count: Int(len)), as: UTF8.self)
          }
          out.append(contentsOf: s.unicodeScalars)
          continue
        }
      }
      out.append(cell.codepoint == 0 ? " " : (Unicode.Scalar(cell.codepoint) ?? " "))
    }
    var s = String(out)
    while s.last == " " { s.removeLast() }
    return s
  }
}
