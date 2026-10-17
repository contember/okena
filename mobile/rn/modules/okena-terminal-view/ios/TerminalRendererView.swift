import UIKit

/// The native terminal renderer (iOS).
///
/// WU2 state: a placeholder. It paints the theme background with a small
/// label, measures a monospace cell from the terminal font and reports the
/// grid size from layout, and turns a single tap into a cell. It does not call
/// Rust yet; WU3/WU4 replace the placeholder with the Core Text row renderer,
/// the scroll model and the gestures (see
/// docs/sprints/sprint-2026-10-17-native-terminal-view.md).
///
/// The Fabric wrapper (`OkenaTerminalViewComponentView.mm`) owns this view,
/// copies props onto it and forwards the `on*` closures to the C++ event
/// emitter. Keep this class free of React Native imports.
@objc(OkenaTerminalRendererView)
public final class TerminalRendererView: UIView {

  // MARK: Props (set by the Fabric wrapper)

  @objc public var connId: String = "" {
    didSet { if connId != oldValue { propsChanged() } }
  }

  @objc public var terminalId: String = "" {
    didSet { if terminalId != oldValue { propsChanged() } }
  }

  /// Font size in pt.
  @objc public var fontSize: CGFloat = 10.5 {
    didSet {
      if fontSize != oldValue {
        metrics = nil
        setNeedsLayout()
      }
    }
  }

  /// Theme colors as packed ARGB (`0xAARRGGBB`), as resolved by the Rust core.
  @objc public var backgroundArgb: UInt32 = 0xff00_0000 {
    didSet { backgroundColor = Self.color(argb: backgroundArgb) }
  }
  @objc public var foregroundArgb: UInt32 = 0xffcd_d6f4 {
    didSet { label.textColor = Self.color(argb: foregroundArgb).withAlphaComponent(0.6) }
  }
  @objc public var cursorArgb: UInt32 = 0xfff5_e0dc
  @objc public var selectionArgb: UInt32 = 0x4026_4f78

  /// Whether this pane is the keyboard / toolbar target.
  @objc public var focusedPane: Bool = false

  // MARK: Events (forwarded to JS by the Fabric wrapper)

  /// Single tap at a cell (col, row).
  @objc public var onTap: ((Int, Int) -> Void)?
  /// Grid geometry: cols, rows, cell width, cell height (pt).
  @objc public var onGridSize: ((Int, Int, Double, Double) -> Void)?
  /// Pinch ended at a new font size (pt).
  @objc public var onFontSizeChange: ((Double) -> Void)?
  /// A selection was copied to `UIPasteboard`.
  @objc public var onSelectionCopied: ((String) -> Void)?
  /// The viewport moved to (true) or away from (false) the live bottom.
  @objc public var onScrollStateChange: ((Bool) -> Void)?

  // MARK: Commands

  @objc public func scrollToBottom() {
    // Placeholder: always at the bottom. WU4 resets the scroll offset here.
    onScrollStateChange?(true)
  }

  @objc public func clearSelection() {
    // Placeholder: no selection yet (WU4).
  }

  // MARK: Layout / measurement

  private struct CellMetrics: Equatable {
    var width: CGFloat
    var height: CGFloat
  }

  private struct Grid: Equatable {
    var cols: Int
    var rows: Int
    var cell: CellMetrics
  }

  private static let lineHeightFactor: CGFloat = 1.2
  private static let maxCols = 500
  private static let maxRows = 200

  private var metrics: CellMetrics?
  private var lastGrid: Grid?
  private let label = UILabel()

  public override init(frame: CGRect) {
    super.init(frame: frame)
    backgroundColor = Self.color(argb: backgroundArgb)
    clipsToBounds = true

    label.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
    label.textColor = Self.color(argb: foregroundArgb).withAlphaComponent(0.6)
    label.numberOfLines = 0
    addSubview(label)

    let tap = UITapGestureRecognizer(target: self, action: #selector(handleTap(_:)))
    addGestureRecognizer(tap)
  }

  required init?(coder: NSCoder) {
    fatalError("init(coder:) is not supported")
  }

  /// Font used for cell metrics: bundled JetBrains Mono, else the system mono.
  private func terminalFont() -> UIFont {
    UIFont(name: "JetBrainsMono-Regular", size: fontSize)
      ?? .monospacedSystemFont(ofSize: fontSize, weight: .regular)
  }

  /// Mirrors `measureCell` in TerminalView.tsx: the advance of "M", and the
  /// ascent + descent times the line-height factor.
  private func measureCell() -> CellMetrics {
    if let metrics { return metrics }
    let font = terminalFont()
    let advance = ("M" as NSString).size(withAttributes: [.font: font]).width
    let textHeight = font.ascender - font.descender
    let measured = CellMetrics(width: advance, height: textHeight * Self.lineHeightFactor)
    metrics = measured
    return measured
  }

  public override func layoutSubviews() {
    super.layoutSubviews()
    label.frame = bounds.insetBy(dx: 12, dy: 12)
    label.sizeToFit()

    let size = bounds.size
    guard size.width > 0, size.height > 0 else { return }
    let cell = measureCell()
    guard cell.width > 0, cell.height > 0 else { return }

    let cols = min(max(Int(size.width / cell.width), 1), Self.maxCols)
    let rows = min(max(Int(size.height / cell.height), 1), Self.maxRows)
    let grid = Grid(cols: cols, rows: rows, cell: cell)
    if grid != lastGrid {
      lastGrid = grid
      updateLabel()
      // WU3: resize_local here, then a 200 ms debounced resize_terminal.
      onGridSize?(cols, rows, Double(cell.width), Double(cell.height))
    }
  }

  private func propsChanged() {
    updateLabel()
  }

  private func updateLabel() {
    var text = "Native terminal (placeholder)\n\(connId) / \(terminalId)"
    if let grid = lastGrid {
      text += "\n\(grid.cols)×\(grid.rows)"
    }
    label.text = text
    setNeedsLayout()
  }

  // MARK: Gestures

  @objc private func handleTap(_ recognizer: UITapGestureRecognizer) {
    guard recognizer.state == .ended else { return }
    let cell = measureCell()
    let point = recognizer.location(in: self)
    let cols = lastGrid?.cols ?? 1
    let rows = lastGrid?.rows ?? 1
    let col = min(max(Int(point.x / cell.width), 0), cols - 1)
    let row = min(max(Int(point.y / cell.height), 0), rows - 1)
    onTap?(col, row)
  }

  // MARK: Helpers

  static func color(argb: UInt32) -> UIColor {
    UIColor(
      red: CGFloat((argb >> 16) & 0xff) / 255,
      green: CGFloat((argb >> 8) & 0xff) / 255,
      blue: CGFloat(argb & 0xff) / 255,
      alpha: CGFloat((argb >> 24) & 0xff) / 255
    )
  }
}
