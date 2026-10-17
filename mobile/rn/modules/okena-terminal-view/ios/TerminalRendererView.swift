import OkenaRender
import UIKit

/// The native terminal renderer (iOS).
///
/// Reads cells straight from the Rust core through the C render ABI
/// (`crates/okena-mobile-ffi/include/okena_render.h`) and draws them with Core
/// Text. See docs/sprints/sprint-2026-10-17-native-terminal-view.md (WU3/WU4).
///
/// - **Scroll host.** A `UIScrollView` provides the physics (momentum, rubber
///   band, status-bar tap). Content row 0 is `first_line`; the content is
///   `(history_size + screen_lines) × cellHeight` tall plus a bottom inset for
///   the part of a row that does not fit, so at the bottom the screen's top row
///   sits at the top edge (as in the Skia view).
/// - **Rows.** One `CALayer` per visible row plus one above and one below,
///   placed in content coordinates. A row bitmap is redrawn only when that
///   row's `(line, hash)` changes; scrolling only moves the scroll view's
///   bounds, so Core Animation moves the layers.
/// - **Frame loop.** A `CADisplayLink` polls the lock-free `generation`.
///   Nothing else runs while it is unchanged and the view is not scrolled.
/// - **Anchoring.** At the live tail the view follows new output. Scrolled up,
///   it keeps the absolute line at its top (shifting when old lines are
///   dropped). A reflow (`reflow_epoch`) keeps the distance from the bottom.
/// - **Overlays.** Cursor, selection and the scroll indicator are separate
///   layers, never baked into row bitmaps.
///
/// The Fabric wrapper (`OkenaTerminalViewComponentView.mm`) owns this view,
/// copies props onto it and forwards the `on*` closures to the C++ event
/// emitter. Keep this class free of React Native imports.
@objc(OkenaTerminalRendererView)
public final class TerminalRendererView: UIView, UIScrollViewDelegate, UIGestureRecognizerDelegate {

  // MARK: Props (set by the Fabric wrapper)

  @objc public var connId: String = "" {
    didSet { if connId != oldValue { reopen() } }
  }

  @objc public var terminalId: String = "" {
    didSet { if terminalId != oldValue { reopen() } }
  }

  /// Font size in pt. Ignored while a pinch is running (the pinch owns it).
  @objc public var fontSize: CGFloat = 10.5 {
    didSet {
      guard fontSize > 0, pinchStartSize == nil, fonts?.size != fontSize else { return }
      applyFontSize()
    }
  }

  /// Theme colors as packed ARGB (`0xAARRGGBB`).
  @objc public var backgroundArgb: UInt32 = 0xff00_0000 {
    didSet {
      guard backgroundArgb != oldValue else { return }
      backgroundColor = Self.color(argb: backgroundArgb)
      invalidateRows()
    }
  }
  /// Unused: Rust resolves cell colors. Kept for the contract.
  @objc public var foregroundArgb: UInt32 = 0xffcd_d6f4
  @objc public var cursorArgb: UInt32 = 0xfff5_e0dc {
    didSet { if cursorArgb != oldValue { needsUpdate = true } }
  }
  @objc public var selectionArgb: UInt32 = 0x4026_4f78 {
    didSet { if selectionArgb != oldValue { needsUpdate = true } }
  }

  /// Whether this pane is the keyboard / toolbar target. The Skia view draws
  /// the cursor the same either way, so this view does too.
  @objc public var focusedPane: Bool = false

  // MARK: Events (forwarded to JS by the Fabric wrapper)

  /// Single tap at a viewport cell (col, row).
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
    following = true
    scrollView.setContentOffset(scrollView.contentOffset, animated: false)  // stop a fling
    setOffset(liveOffset)
    needsUpdate = true
    reportScrollState()
  }

  @objc public func clearSelection() {
    guard let handle else { return }
    okena_rv_selection_clear(handle)
    needsUpdate = true
  }

  // MARK: Constants

  private static let maxCols = 500
  private static let maxRows = 200
  private static let minFontSize: CGFloat = 8
  private static let maxFontSize: CGFloat = 20
  private static let resizeDebounce: TimeInterval = 0.2

  // MARK: State

  private var handle: OpaquePointer?
  private var fonts: TerminalFonts?
  private var painter: TerminalRowPainter?

  private let scrollView = TerminalScrollView()
  private let contentView = UIView()
  private let cursorLayer = TerminalLayer()
  private let selectionLayers = [TerminalLayer(), TerminalLayer(), TerminalLayer()]
  private let indicatorLayer = TerminalLayer()

  private struct RowSlot {
    let layer: TerminalLayer
    var hash: UInt64
  }
  /// Row layers by absolute line.
  private var rows: [Int64: RowSlot] = [:]
  private var spareLayers: [TerminalLayer] = []

  // Header values of the frame the layout is built on.
  private var lastGeneration: UInt64 = .max
  private var firstLine: Int64 = 0
  private var totalLines: Int = 0
  private var screenLines: Int = 0
  private var frameCols: Int = 0
  private var reflowEpoch: UInt32 = 0
  private var altScreen = false
  private var haveFrame = false
  private var missing = false
  private var consumesWheel = false
  private var windowTop: Int64 = .min

  /// True while the view sticks to the live bottom.
  private var following = true
  private var reportedAtBottom: Bool?
  private var adjustingOffset = false
  /// Set when something other than `generation` needs a new snapshot.
  private var needsUpdate = true
  /// Lines from the bottom to restore after a font change re-layout.
  private var pendingLinesFromBottom: CGFloat?

  // Sizing.
  private struct Grid: Equatable {
    var cols: Int
    var rows: Int
    var cellWidth: CGFloat
    var cellHeight: CGFloat
  }
  private var grid: Grid?
  private var initialResizeSent = false
  private var resizeWork: DispatchWorkItem?

  // Frame loop.
  private var displayLink: CADisplayLink?
  private var inBackground = false

  // Gestures.
  private let tapRecognizer = UITapGestureRecognizer()
  private let doubleTapRecognizer = UITapGestureRecognizer()
  private let longPressRecognizer = UILongPressGestureRecognizer()
  private let pinchRecognizer = UIPinchGestureRecognizer()
  private let wheelPanRecognizer = UIPanGestureRecognizer()
  private var wheelAccumulator: CGFloat = 0
  private var pinchStartSize: CGFloat?
  private var livePinchSize: CGFloat = 10.5
  private var selecting = false
  private var selectionPoint: CGPoint = .zero
  private var autoScrollSpeed: CGFloat = 0
  private var clearSelectionWork: DispatchWorkItem?

  // Accessibility cache.
  private var accessibilityCache: (generation: UInt64, top: Int64, text: String)?

  // Perf sampling (opt-in, see `PerfLog`).
  private let perf = PerfLog()

  // MARK: Init

  public override init(frame: CGRect) {
    super.init(frame: frame)
    backgroundColor = Self.color(argb: backgroundArgb)
    clipsToBounds = true

    scrollView.delegate = self
    scrollView.accessibilityLabelProvider = { [weak self] in self?.visibleText() }
    scrollView.backgroundColor = .clear
    scrollView.showsVerticalScrollIndicator = false
    scrollView.showsHorizontalScrollIndicator = false
    scrollView.alwaysBounceVertical = true
    scrollView.contentInsetAdjustmentBehavior = .never
    scrollView.scrollsToTop = true
    scrollView.panGestureRecognizer.maximumNumberOfTouches = 1
    scrollView.delaysContentTouches = false
    addSubview(scrollView)
    scrollView.addSubview(contentView)
    contentView.isUserInteractionEnabled = false

    for layer in selectionLayers {
      layer.isHidden = true
      layer.zPosition = 1
      contentView.layer.addSublayer(layer)
    }
    cursorLayer.isHidden = true
    cursorLayer.zPosition = 2
    contentView.layer.addSublayer(cursorLayer)
    indicatorLayer.isHidden = true
    indicatorLayer.cornerRadius = 1.5
    indicatorLayer.backgroundColor = Self.color(argb: 0x40ff_ffff).cgColor
    layer.addSublayer(indicatorLayer)

    tapRecognizer.addTarget(self, action: #selector(handleTap(_:)))
    tapRecognizer.delegate = self
    addGestureRecognizer(tapRecognizer)

    doubleTapRecognizer.numberOfTapsRequired = 2
    doubleTapRecognizer.addTarget(self, action: #selector(handleDoubleTap(_:)))
    doubleTapRecognizer.delegate = self
    addGestureRecognizer(doubleTapRecognizer)

    longPressRecognizer.minimumPressDuration = 0.35
    longPressRecognizer.addTarget(self, action: #selector(handleLongPress(_:)))
    longPressRecognizer.delegate = self
    addGestureRecognizer(longPressRecognizer)

    pinchRecognizer.addTarget(self, action: #selector(handlePinch(_:)))
    pinchRecognizer.delegate = self
    addGestureRecognizer(pinchRecognizer)

    wheelPanRecognizer.maximumNumberOfTouches = 1
    wheelPanRecognizer.addTarget(self, action: #selector(handleWheelPan(_:)))
    wheelPanRecognizer.delegate = self
    addGestureRecognizer(wheelPanRecognizer)


    let center = NotificationCenter.default
    center.addObserver(
      self, selector: #selector(didEnterBackground), name: UIApplication.didEnterBackgroundNotification,
      object: nil)
    center.addObserver(
      self, selector: #selector(willEnterForeground), name: UIApplication.willEnterForegroundNotification,
      object: nil)
  }

  required init?(coder: NSCoder) {
    fatalError("init(coder:) is not supported")
  }

  deinit {
    displayLink?.invalidate()
    resizeWork?.cancel()
    clearSelectionWork?.cancel()
    if let handle { okena_rv_close(handle) }
  }

  // MARK: Lifecycle

  private func reopen() {
    if let handle { okena_rv_close(handle) }
    handle = nil
    resetTerminalState()
    guard !connId.isEmpty, !terminalId.isEmpty else { return }
    handle = connId.withCString { c in terminalId.withCString { t in okena_rv_open(c, t) } }
    // The (new) terminal gets its size from this view.
    grid = nil
    initialResizeSent = false
    setNeedsLayout()
  }

  private func resetTerminalState() {
    resizeWork?.cancel()
    resizeWork = nil
    clearSelectionWork?.cancel()
    for (_, slot) in rows { recycle(slot.layer) }
    rows.removeAll()
    lastGeneration = .max
    firstLine = 0
    totalLines = 0
    screenLines = 0
    frameCols = 0
    reflowEpoch = 0
    altScreen = false
    haveFrame = false
    missing = false
    consumesWheel = false
    windowTop = .min
    following = true
    selecting = false
    autoScrollSpeed = 0
    needsUpdate = true
    accessibilityCache = nil
    cursorLayer.isHidden = true
    selectionLayers.forEach { $0.isHidden = true }
    indicatorLayer.isHidden = true
    scrollView.isScrollEnabled = true
    updateContentGeometry()
    setOffset(0)
  }

  public override func didMoveToWindow() {
    super.didMoveToWindow()
    if window != nil {
      if displayLink == nil {
        let link = CADisplayLink(target: DisplayLinkProxy(self), selector: #selector(DisplayLinkProxy.tick(_:)))
        link.add(to: .main, forMode: .common)
        displayLink = link
      }
      displayLink?.isPaused = inBackground
      needsUpdate = true
      if let fonts, abs(fonts.scale - screenScale) > 0.01 { applyFontSize() }
    } else {
      displayLink?.invalidate()
      displayLink = nil
    }
  }

  @objc private func didEnterBackground() {
    inBackground = true
    displayLink?.isPaused = true
  }

  @objc private func willEnterForeground() {
    inBackground = false
    displayLink?.isPaused = window == nil
    needsUpdate = true
  }

  private var screenScale: CGFloat {
    window?.screen.scale ?? UIScreen.main.scale
  }

  // MARK: Fonts and sizing

  /// Rebuild fonts and rows at the current (or live pinch) size.
  private func applyFontSize() {
    if let fonts, haveFrame {
      pendingLinesFromBottom = (liveOffset - scrollView.contentOffset.y) / fonts.cellHeight
    }
    fonts = nil
    painter = nil
    invalidateRows()
    setNeedsLayout()
    layoutIfNeeded()
  }

  private func ensureFonts() -> TerminalFonts {
    if let fonts { return fonts }
    let made = TerminalFonts(size: pinchStartSize != nil ? livePinchSize : fontSize, scale: screenScale)
    fonts = made
    return made
  }

  private func invalidateRows() {
    for (_, slot) in rows { recycle(slot.layer) }
    rows.removeAll()
    needsUpdate = true
  }

  public override func layoutSubviews() {
    super.layoutSubviews()
    let size = bounds.size
    guard size.width > 0, size.height > 0 else { return }

    var distanceFromBottom: CGFloat = 0
    if fonts != nil, haveFrame { distanceFromBottom = liveOffset - scrollView.contentOffset.y }
    if scrollView.frame != bounds {
      adjustingOffset = true
      scrollView.frame = bounds
      adjustingOffset = false
    }
    let fonts = ensureFonts()
    if let lines = pendingLinesFromBottom {
      distanceFromBottom = lines * fonts.cellHeight
      pendingLinesFromBottom = nil
    }
    if painter == nil || painter?.widthPt != size.width || painter?.fonts !== fonts {
      painter = TerminalRowPainter(fonts: fonts, width: size.width)
      invalidateRows()
    }

    let cols = min(max(Int(size.width / fonts.cellWidth), 1), Self.maxCols)
    let rowCount = min(max(Int(size.height / fonts.cellHeight), 1), Self.maxRows)
    let newGrid = Grid(cols: cols, rows: rowCount, cellWidth: fonts.cellWidth, cellHeight: fonts.cellHeight)
    if newGrid != grid {
      let sizeChanged = grid?.cols != cols || grid?.rows != rowCount
      grid = newGrid
      if sizeChanged { resize(cols: cols, rows: rowCount) }
      onGridSize?(cols, rowCount, Double(fonts.cellWidth), Double(fonts.cellHeight))
    }

    updateContentGeometry()
    // Keep the distance from the bottom across a layout change.
    if following {
      setOffset(liveOffset)
    } else if haveFrame {
      setOffset(min(max(liveOffset - distanceFromBottom, 0), maxOffset))
    }
    needsUpdate = true
    updateViewport()
  }

  /// `resize_local` at once; `resize_terminal` at once the first time, then
  /// debounced (200 ms), as in TerminalView.tsx.
  private func resize(cols: Int, rows: Int) {
    guard let handle else { return }
    let c = UInt16(cols)
    let r = UInt16(rows)
    okena_rv_resize_local(handle, c, r)
    resizeWork?.cancel()
    if !initialResizeSent {
      initialResizeSent = true
      okena_rv_resize_terminal(handle, c, r)
    } else {
      let work = DispatchWorkItem { [weak self] in
        guard let self, let handle = self.handle else { return }
        okena_rv_resize_terminal(handle, c, r)
      }
      resizeWork = work
      DispatchQueue.main.asyncAfter(deadline: .now() + Self.resizeDebounce, execute: work)
    }
  }

  // MARK: Scroll geometry

  private var bottomInset: CGFloat {
    guard let fonts, let grid else { return 0 }
    return max(bounds.height - CGFloat(grid.rows) * fonts.cellHeight, 0)
  }

  private var maxOffset: CGFloat {
    guard let fonts else { return 0 }
    return max(CGFloat(totalLines) * fonts.cellHeight + bottomInset - bounds.height, 0)
  }

  /// The live-tail offset: the screen's top row at the top edge, as the Skia
  /// view draws it. Normally this is `maxOffset`; when another client made
  /// the PTY taller than this view, the rest of the screen is reachable by
  /// scrolling further down.
  private var liveOffset: CGFloat {
    guard let fonts else { return 0 }
    return min(CGFloat(max(totalLines - screenLines, 0)) * fonts.cellHeight, maxOffset)
  }

  private func updateContentGeometry() {
    let height = CGFloat(totalLines) * (fonts?.cellHeight ?? 0)
    let size = CGSize(width: bounds.width, height: height)
    adjustingOffset = true
    if scrollView.contentSize != size { scrollView.contentSize = size }
    let inset = UIEdgeInsets(top: 0, left: 0, bottom: bottomInset, right: 0)
    if scrollView.contentInset != inset { scrollView.contentInset = inset }
    // Row layers may lie outside the content view's bounds (it does not
    // clip), so its frame only follows the width: resizing a UIView on every
    // streamed line showed up in profiles.
    let contentFrame = CGRect(x: 0, y: 0, width: bounds.width, height: bounds.height)
    if contentView.frame != contentFrame { contentView.frame = contentFrame }
    adjustingOffset = false
  }

  private func setOffset(_ y: CGFloat) {
    guard scrollView.contentOffset.y != y else { return }
    adjustingOffset = true
    scrollView.contentOffset = CGPoint(x: 0, y: y)
    adjustingOffset = false
  }

  private var atBottom: Bool {
    guard let fonts else { return true }
    return scrollView.contentOffset.y >= liveOffset - fonts.cellHeight / 2
  }

  private func reportScrollState() {
    let value = following || atBottom
    if value != reportedAtBottom {
      reportedAtBottom = value
      onScrollStateChange?(value)
    }
  }

  // MARK: UIScrollViewDelegate

  public func scrollViewDidScroll(_ scrollView: UIScrollView) {
    guard !adjustingOffset else { return }
    if scrollView.isTracking || scrollView.isDecelerating || scrollView.isDragging {
      following = atBottom
    }
    updateIndicator()
    reportScrollState()
    if currentWindow().top != windowTop { updateViewport() }
  }

  public func scrollViewDidEndDragging(_ scrollView: UIScrollView, willDecelerate decelerate: Bool) {
    if !decelerate {
      following = atBottom
      reportScrollState()
    }
  }

  public func scrollViewDidEndDecelerating(_ scrollView: UIScrollView) {
    following = atBottom
    reportScrollState()
  }

  public func scrollViewDidScrollToTop(_ scrollView: UIScrollView) {
    following = atBottom
    reportScrollState()
  }

  // MARK: Frame loop

  fileprivate func tick(_ link: CADisplayLink) {
    if selecting && autoScrollSpeed != 0 { autoScroll() }
    guard let handle else { return }
    let generation = okena_rv_generation(handle)
    if generation != lastGeneration || needsUpdate {
      updateViewport()
    }
  }

  /// Lines to read: one above the viewport, the visible ones, one below.
  private func currentWindow() -> (top: Int64, count: Int) {
    guard let fonts else { return (0, 1) }
    let ch = fonts.cellHeight
    let topRow = Int64((scrollView.contentOffset.y / ch).rounded(.down))
    let visible = Int((bounds.height / ch).rounded(.up)) + 1
    return (firstLine + topRow - 1, min(visible + 2, 1024))
  }

  private func updateViewport() {
    guard let handle, let fonts, let painter, bounds.width > 0 else { return }
    let start = perf.enabled ? CACurrentMediaTime() : 0
    needsUpdate = false

    var window = currentWindow()
    guard var frame = okena_rv_snapshot(handle, window.top, UInt16(window.count)) else { return }
    if frame.pointee.status == OKENA_RV_STATUS_MISSING {
      showMissing(generation: frame.pointee.generation)
      return
    }
    if !haveFrame, let grid {
      // The terminal just appeared (or the view was just opened): make sure
      // its local grid has this view's size.
      okena_rv_resize_local(handle, UInt16(grid.cols), UInt16(grid.rows))
      if missing {
        okena_rv_resize_terminal(handle, UInt16(grid.cols), UInt16(grid.rows))
      }
    }
    missing = false

    if applyHeader(frame.pointee, fonts: fonts) {
      // The anchor moved: read the window at the new position.
      window = currentWindow()
      guard let again = okena_rv_snapshot(handle, window.top, UInt16(window.count)) else { return }
      frame = again
      _ = applyHeader(frame.pointee, fonts: fonts)
    }
    windowTop = window.top
    lastGeneration = frame.pointee.generation

    // Rows.
    let f = frame.pointee
    let rowMeta = okena_rv_frame_rows(frame)!
    var keep = Set<Int64>(minimumCapacity: Int(f.rows))
    var redrawn = 0
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    for r in 0..<Int(f.rows) {
      let meta = rowMeta[r]
      if meta.flags & UInt32(OKENA_RV_ROW_VOID) != 0 { continue }
      let line = meta.line
      keep.insert(line)
      let y = CGFloat(line - firstLine) * fonts.cellHeight
      let rowFrame = CGRect(x: 0, y: y, width: painter.widthPt, height: fonts.cellHeight)
      if var slot = rows[line] {
        if slot.hash != meta.hash {
          slot.layer.contents = painter.paint(frame: frame, row: UInt16(r), background: backgroundArgb)
          slot.hash = meta.hash
          rows[line] = slot
          redrawn += 1
        }
        if slot.layer.frame != rowFrame { slot.layer.frame = rowFrame }
      } else {
        let layer = dequeueLayer()
        layer.contents = painter.paint(frame: frame, row: UInt16(r), background: backgroundArgb)
        layer.frame = rowFrame
        layer.isHidden = false
        rows[line] = RowSlot(layer: layer, hash: meta.hash)
        redrawn += 1
      }
    }
    for (line, slot) in rows where !keep.contains(line) {
      recycle(slot.layer)
      rows[line] = nil
    }
    updateCursor(f, fonts: fonts)
    updateSelection(f, fonts: fonts)
    updateIndicator()
    CATransaction.commit()

    accessibilityCache = nil
    reportScrollState()
    if perf.enabled { perf.record(ms: (CACurrentMediaTime() - start) * 1000, redrawn: redrawn) }
  }

  /// Apply a frame header: content size, anchoring, mode. Returns true when
  /// the scroll position moved so that the snapshot window is stale.
  private func applyHeader(_ f: OkenaRvFrame, fonts: TerminalFonts) -> Bool {
    let oldLive = liveOffset
    let newFirst = f.first_line
    let newTotal = Int(f.history_size) + Int(f.screen_lines)
    let newAlt = f.mode & UInt32(OKENA_RV_MODE_ALT_SCREEN) != 0
    let wheelMode =
      f.mode & UInt32(OKENA_RV_MODE_MOUSE) != 0
      || (newAlt && f.mode & UInt32(OKENA_RV_MODE_ALTERNATE_SCROLL) != 0)
    screenLines = Int(f.screen_lines)
    frameCols = Int(f.cols)

    if wheelMode != consumesWheel {
      consumesWheel = wheelMode
      // The app scrolls itself: the scroll view must not move.
      scrollView.isScrollEnabled = !wheelMode
    }

    let changed =
      !haveFrame || newFirst != firstLine || newTotal != totalLines || f.reflow_epoch != reflowEpoch
      || newAlt != altScreen
    guard changed else { return false }

    let oldOffset = scrollView.contentOffset.y
    let oldFirst = firstLine
    let hadFrame = haveFrame
    let reflowed = hadFrame && f.reflow_epoch != reflowEpoch
    if newAlt != altScreen { following = true }

    firstLine = newFirst
    totalLines = newTotal
    reflowEpoch = f.reflow_epoch
    altScreen = newAlt
    haveFrame = true
    updateContentGeometry()

    let newMax = maxOffset
    let newLive = liveOffset
    var target: CGFloat
    if following || !hadFrame {
      target = newLive
    } else if reflowed {
      target = newLive - (oldLive - oldOffset)
    } else {
      // Keep the same absolute line at the top.
      target = oldOffset - CGFloat(newFirst - oldFirst) * fonts.cellHeight
    }
    if scrollView.isTracking || scrollView.isDecelerating {
      target = max(target, -bounds.height / 2)
    } else {
      target = min(max(target, 0), newMax)
    }
    if newFirst != oldFirst {
      // Content row 0 moved: reposition every cached row.
      for (line, slot) in rows {
        slot.layer.frame.origin.y = CGFloat(line - newFirst) * fonts.cellHeight
      }
    }
    setOffset(target)
    return currentWindow().top != windowTop
  }

  private func showMissing(generation: UInt64) {
    lastGeneration = generation
    guard !missing || haveFrame else { return }
    missing = true
    haveFrame = false
    totalLines = 0
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    for (_, slot) in rows { recycle(slot.layer) }
    rows.removeAll()
    cursorLayer.isHidden = true
    selectionLayers.forEach { $0.isHidden = true }
    indicatorLayer.isHidden = true
    CATransaction.commit()
    updateContentGeometry()
    setOffset(0)
  }

  private func dequeueLayer() -> TerminalLayer {
    if let layer = spareLayers.popLast() { return layer }
    let layer = TerminalLayer()
    layer.isOpaque = true
    layer.contentsScale = fonts?.scale ?? screenScale
    contentView.layer.insertSublayer(layer, at: 0)
    return layer
  }

  private func recycle(_ layer: TerminalLayer) {
    layer.isHidden = true
    layer.contents = nil
    if let fonts { layer.contentsScale = fonts.scale }
    spareLayers.append(layer)
  }

  // MARK: Overlays

  private func updateCursor(_ f: OkenaRvFrame, fonts: TerminalFonts) {
    guard f.cursor_visible != 0, f.cursor_col < f.cols else {
      cursorLayer.isHidden = true
      return
    }
    let cw = fonts.cellWidth
    let ch = fonts.cellHeight
    let x = CGFloat(f.cursor_col) * cw
    let y = CGFloat(f.cursor_line - firstLine) * ch
    let color = cursorArgb
    switch UInt32(f.cursor_shape) {
    case OKENA_RV_CURSOR_BEAM:
      cursorLayer.frame = CGRect(x: x - 1, y: y, width: 2, height: ch)
      cursorLayer.backgroundColor = Self.color(argb: color).cgColor
      cursorLayer.borderWidth = 0
    case OKENA_RV_CURSOR_UNDERLINE:
      cursorLayer.frame = CGRect(x: x, y: y + ch - 2, width: cw, height: 2)
      cursorLayer.backgroundColor = Self.color(argb: color).cgColor
      cursorLayer.borderWidth = 0
    case OKENA_RV_CURSOR_HOLLOW_BLOCK:
      cursorLayer.frame = CGRect(x: x, y: y, width: cw, height: ch)
      cursorLayer.backgroundColor = nil
      cursorLayer.borderColor = Self.color(argb: color).cgColor
      cursorLayer.borderWidth = 1
    default:
      // Half-alpha block, as in the Skia view.
      cursorLayer.frame = CGRect(x: x, y: y, width: cw, height: ch)
      cursorLayer.backgroundColor = Self.color(argb: 0x8000_0000 | (color & 0x00ff_ffff)).cgColor
      cursorLayer.borderWidth = 0
    }
    cursorLayer.isHidden = false
  }

  private func updateSelection(_ f: OkenaRvFrame, fonts: TerminalFonts) {
    let kind = UInt32(f.sel_kind)
    guard kind != OKENA_RV_SEL_NONE else {
      selectionLayers.forEach { $0.isHidden = true }
      return
    }
    let cw = fonts.cellWidth
    let ch = fonts.cellHeight
    let cols = CGFloat(f.cols)
    let sl = CGFloat(f.sel_start_line - firstLine)
    let el = CGFloat(f.sel_end_line - firstLine)
    let sc = CGFloat(f.sel_start_col)
    let ec = CGFloat(f.sel_end_col)
    var rects: [CGRect] = []
    if kind == OKENA_RV_SEL_BLOCK {
      let c0 = min(sc, ec)
      let c1 = max(sc, ec)
      rects.append(CGRect(x: c0 * cw, y: sl * ch, width: (c1 - c0 + 1) * cw, height: (el - sl + 1) * ch))
    } else if sl == el {
      rects.append(CGRect(x: sc * cw, y: sl * ch, width: (ec - sc + 1) * cw, height: ch))
    } else {
      rects.append(CGRect(x: sc * cw, y: sl * ch, width: (cols - sc) * cw, height: ch))
      if el - sl > 1 {
        rects.append(CGRect(x: 0, y: (sl + 1) * ch, width: cols * cw, height: (el - sl - 1) * ch))
      }
      rects.append(CGRect(x: 0, y: el * ch, width: (ec + 1) * cw, height: ch))
    }
    let color = Self.color(argb: selectionArgb).cgColor
    for (i, layer) in selectionLayers.enumerated() {
      if i < rects.count, rects[i].width > 0, rects[i].height > 0 {
        layer.frame = rects[i]
        layer.backgroundColor = color
        layer.isHidden = false
      } else {
        layer.isHidden = true
      }
    }
  }

  /// The scrollback thumb: 3 pt wide, 4 pt from the right edge, at least 20 pt
  /// tall, as in the Skia view.
  private func updateIndicator() {
    guard haveFrame, screenLines > 0, totalLines > screenLines else {
      indicatorLayer.isHidden = true
      return
    }
    let track = bounds.height
    let thumb = min(max(CGFloat(screenLines) / CGFloat(totalLines) * track, 20), track)
    let live = liveOffset
    let progress = live > 0 ? min(max(scrollView.contentOffset.y / live, 0), 1) : 1
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    indicatorLayer.frame = CGRect(x: bounds.width - 4, y: progress * (track - thumb), width: 3, height: thumb)
    indicatorLayer.isHidden = false
    CATransaction.commit()
  }

  // MARK: Hit testing helpers

  /// Absolute line, column and side under a point in this view.
  private func cell(at point: CGPoint) -> (line: Int64, col: UInt16, side: UInt8)? {
    guard let fonts else { return nil }
    let y = point.y + scrollView.contentOffset.y
    let line = firstLine + Int64((y / fonts.cellHeight).rounded(.down))
    let cols = max(frameCols, grid?.cols ?? 1)
    let raw = max(point.x, 0) / fonts.cellWidth
    let col = min(max(Int(raw.rounded(.down)), 0), cols - 1)
    let side = raw - CGFloat(col) < 0.5 ? UInt8(OKENA_RV_SIDE_LEFT) : UInt8(OKENA_RV_SIDE_RIGHT)
    return (line, UInt16(col), side)
  }

  // MARK: Gestures

  public func gestureRecognizer(
    _ gestureRecognizer: UIGestureRecognizer,
    shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer
  ) -> Bool {
    // A tap does not wait for a possible double tap (it focuses at once, as in
    // the Skia view); the double tap then selects a word.
    if gestureRecognizer === tapRecognizer && other === doubleTapRecognizer { return true }
    if gestureRecognizer === doubleTapRecognizer && other === tapRecognizer { return true }
    return false
  }

  public override func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
    if gestureRecognizer === wheelPanRecognizer { return consumesWheel && handle != nil }
    return super.gestureRecognizerShouldBegin(gestureRecognizer)
  }

  @objc private func handleTap(_ recognizer: UITapGestureRecognizer) {
    guard recognizer.state == .ended, let fonts else { return }
    let point = recognizer.location(in: self)
    let cols = grid?.cols ?? 1
    let rowCount = grid?.rows ?? 1
    let col = min(max(Int(point.x / fonts.cellWidth), 0), cols - 1)
    let row = min(max(Int(point.y / fonts.cellHeight), 0), rowCount - 1)
    onTap?(col, row)
  }

  @objc private func handleDoubleTap(_ recognizer: UITapGestureRecognizer) {
    guard recognizer.state == .ended, let handle, let hit = cell(at: recognizer.location(in: self)) else { return }
    okena_rv_select_word(handle, hit.line, hit.col)
    copySelection()
    // Keep the word highlighted for a moment so the copy is visible.
    needsUpdate = true
    clearSelectionWork?.cancel()
    let work = DispatchWorkItem { [weak self] in
      guard let self, !self.selecting else { return }
      self.clearSelection()
    }
    clearSelectionWork = work
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.35, execute: work)
  }

  @objc private func handleLongPress(_ recognizer: UILongPressGestureRecognizer) {
    guard let handle else { return }
    let point = recognizer.location(in: self)
    switch recognizer.state {
    case .began:
      guard let hit = cell(at: point) else { return }
      clearSelectionWork?.cancel()
      selecting = true
      selectionPoint = point
      okena_rv_selection_start(handle, hit.line, hit.col, hit.side, UInt8(OKENA_RV_SELECT_SIMPLE))
      needsUpdate = true
    case .changed:
      selectionPoint = point
      extendSelection()
      updateAutoScroll(point)
    case .ended, .cancelled, .failed:
      guard selecting else { return }
      selecting = false
      autoScrollSpeed = 0
      copySelection()
      okena_rv_selection_clear(handle)
      needsUpdate = true
    default:
      break
    }
  }

  private func extendSelection() {
    guard let handle, let hit = cell(at: selectionPoint) else { return }
    okena_rv_selection_update(handle, hit.line, hit.col, hit.side)
    needsUpdate = true
  }

  /// Edge auto-scroll: near the top or bottom edge, scroll at a speed that
  /// grows towards (and past) the edge.
  private func updateAutoScroll(_ point: CGPoint) {
    guard let fonts, !consumesWheel else {
      autoScrollSpeed = 0
      return
    }
    let zone = fonts.cellHeight * 1.5
    if point.y < zone {
      autoScrollSpeed = -min((zone - point.y) / zone, 3) * fonts.cellHeight * 0.5
    } else if point.y > bounds.height - zone {
      autoScrollSpeed = min((point.y - (bounds.height - zone)) / zone, 3) * fonts.cellHeight * 0.5
    } else {
      autoScrollSpeed = 0
    }
  }

  private func autoScroll() {
    let y = min(max(scrollView.contentOffset.y + autoScrollSpeed, 0), maxOffset)
    guard y != scrollView.contentOffset.y else { return }
    setOffset(y)
    following = atBottom
    updateIndicator()
    reportScrollState()
    extendSelection()
  }

  private func copySelection() {
    guard let handle else { return }
    var len = 0
    guard let ptr = okena_rv_selection_text(handle, &len), len > 0 else { return }
    let text = String(cString: ptr)
    guard !text.isEmpty else { return }
    UIPasteboard.general.string = text
    if perf.enabled { NSLog("DBG ntv copied %d chars: %@", text.count, String(text.prefix(200))) }
    onSelectionCopied?(text)
  }

  @objc private func handlePinch(_ recognizer: UIPinchGestureRecognizer) {
    switch recognizer.state {
    case .began:
      pinchStartSize = fontSize
      livePinchSize = fontSize
    case .changed:
      guard let start = pinchStartSize else { return }
      let size = Self.clampFontSize(start * recognizer.scale)
      if size != livePinchSize {
        livePinchSize = size
        applyFontSize()
      }
    case .ended, .cancelled, .failed:
      guard let start = pinchStartSize else { return }
      let final = livePinchSize
      pinchStartSize = nil
      if final != start {
        // Adopt the size right away; JS persists it and sends it back as the
        // prop (then a no-op).
        fontSize = final
        onFontSizeChange?(Double(final))
      } else if fonts?.size != fontSize {
        applyFontSize()
      }
    default:
      break
    }
  }

  /// 8–20 pt in steps of 0.5, as in the settings.
  static func clampFontSize(_ size: CGFloat) -> CGFloat {
    let stepped = (size * 2).rounded() / 2
    return min(max(stepped, minFontSize), maxFontSize)
  }

  /// Mouse mode / alternate scroll: the pan becomes discrete wheel notches
  /// (finger down = positive = back in history, as in terminalScroll.ts).
  @objc private func handleWheelPan(_ recognizer: UIPanGestureRecognizer) {
    guard let handle, let fonts else { return }
    switch recognizer.state {
    case .began:
      wheelAccumulator = 0
      recognizer.setTranslation(.zero, in: self)
    case .changed:
      wheelAccumulator += recognizer.translation(in: self).y
      recognizer.setTranslation(.zero, in: self)
      let lines = Int((wheelAccumulator / fonts.cellHeight).rounded(.towardZero))
      guard lines != 0 else { return }
      wheelAccumulator -= CGFloat(lines) * fonts.cellHeight
      let point = recognizer.location(in: self)
      let cols = max(frameCols, 1)
      let col = min(max(Int(point.x / fonts.cellWidth), 0), cols - 1)
      let row = min(max(Int(point.y / fonts.cellHeight), 0), max(screenLines - 1, 0))
      _ = okena_rv_wheel(handle, Int32(lines), UInt16(col), UInt16(row))
    default:
      wheelAccumulator = 0
    }
  }

  // MARK: Accessibility (basic: the visible lines as text)
  //
  // One element whose label is the visible lines, one per line (VoiceOver
  // reads it; UI tests can match on it). The Fabric host view
  // becomes the element (it mirrors `isAccessibilityElement` of this content
  // view) and builds its label from the labels of this view's subviews, so the
  // scroll view reports the text too.

  public override var isAccessibilityElement: Bool {
    get { haveFrame }
    set {}
  }

  public override var accessibilityTraits: UIAccessibilityTraits {
    get { .staticText }
    set {}
  }

  public override var accessibilityLabel: String? {
    get { visibleText() }
    set {}
  }

  /// The text of the rows inside the viewport, trailing blanks trimmed.
  private func visibleText() -> String? {
    guard let handle, let fonts, haveFrame else { return nil }
    let window = currentWindow()
    if let cache = accessibilityCache, cache.generation == lastGeneration, cache.top == window.top {
      return cache.text
    }
    guard let frame = okena_rv_snapshot(handle, window.top, UInt16(window.count)) else { return nil }
    var lines: [String] = []
    let rowMeta = okena_rv_frame_rows(frame)!
    for r in 0..<Int(frame.pointee.rows) {
      let meta = rowMeta[r]
      if meta.flags & UInt32(OKENA_RV_ROW_VOID) != 0 { continue }
      let y = CGFloat(meta.line - firstLine) * fonts.cellHeight - scrollView.contentOffset.y
      guard y + fonts.cellHeight > 0, y < bounds.height else { continue }
      lines.append(TerminalRowPainter.text(frame: frame, row: UInt16(r)))
    }
    while lines.last?.isEmpty == true { lines.removeLast() }
    let text = lines.joined(separator: "\n")
    accessibilityCache = (lastGeneration, window.top, text)
    return text
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

/// The scroll host. Reports the terminal text as its accessibility label for
/// the Fabric host view, which reads the labels of its content's subviews.
final class TerminalScrollView: UIScrollView {
  var accessibilityLabelProvider: (() -> String?)?

  override var accessibilityLabel: String? {
    get { accessibilityLabelProvider?() }
    set {}
  }
}

/// A layer without implicit animations.
final class TerminalLayer: CALayer {
  override func action(forKey event: String) -> CAAction? { nil }
}

/// Breaks the CADisplayLink → target retain cycle.
private final class DisplayLinkProxy: NSObject {
  weak var view: TerminalRendererView?
  init(_ view: TerminalRendererView) { self.view = view }
  @objc func tick(_ link: CADisplayLink) {
    if let view { view.tick(link) } else { link.invalidate() }
  }
}

/// Opt-in timing of `updateViewport` on the main thread, for the run log:
///   xcrun simctl spawn booted defaults write com.contember.okena OkenaTerminalPerfLog -bool YES
/// Logs `DBG ntv` lines with p50 / p95 / max over windows of 120 updates, and
/// (see `MainThreadMonitor`) the main thread's busy time per run-loop pass and
/// the display-link frame intervals.
private final class PerfLog {
  let enabled = UserDefaults.standard.bool(forKey: "OkenaTerminalPerfLog")

  init() {
    if enabled { MainThreadMonitor.start() }
  }
  private var samples: [Double] = []
  private var redrawn = 0
  private var windowStart = CACurrentMediaTime()

  func record(ms: Double, redrawn rows: Int) {
    samples.append(ms)
    redrawn += rows
    guard samples.count >= 120 else { return }
    let sorted = samples.sorted()
    let p50 = sorted[sorted.count / 2]
    let p95 = sorted[Int(Double(sorted.count) * 0.95)]
    let elapsed = CACurrentMediaTime() - windowStart
    NSLog(
      "DBG ntv update n=%d p50=%.3f p95=%.3f max=%.3f ms rows=%d over %.2fs", sorted.count, p50, p95,
      sorted.last ?? 0, redrawn, elapsed)
    samples.removeAll(keepingCapacity: true)
    redrawn = 0
    windowStart = CACurrentMediaTime()
  }
}

/// Perf-log only: measures every main run-loop pass (after waking until about
/// to sleep, which includes UIKit layout and the Core Animation commit) and
/// the intervals between display-link frames. Logs a `DBG ntv main` line every
/// 3 s while there was work. Simulator numbers are not device numbers.
private final class MainThreadMonitor: NSObject {
  private static var shared: MainThreadMonitor?
  private var observer: CFRunLoopObserver?
  private var link: CADisplayLink?
  private var wake: CFTimeInterval = 0
  private var busy: [Double] = []
  private var deltas: [Double] = []
  private var last: CFTimeInterval = 0
  private var windowStart = CACurrentMediaTime()

  static func start() {
    guard shared == nil else { return }
    let monitor = MainThreadMonitor()
    shared = monitor
    monitor.install()
  }

  private func install() {
    let activities = CFRunLoopActivity.afterWaiting.rawValue | CFRunLoopActivity.beforeWaiting.rawValue
    observer = CFRunLoopObserverCreateWithHandler(nil, activities, true, Int.max) { [weak self] _, activity in
      guard let self else { return }
      let now = CACurrentMediaTime()
      if activity == .afterWaiting {
        self.wake = now
      } else if self.wake > 0 {
        self.busy.append((now - self.wake) * 1000)
        self.wake = 0
      }
    }
    CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .commonModes)
    let link = CADisplayLink(target: self, selector: #selector(frame(_:)))
    link.add(to: .main, forMode: .common)
    self.link = link
  }

  @objc private func frame(_ link: CADisplayLink) {
    let now = link.timestamp
    if last > 0 { deltas.append((now - last) * 1000) }
    last = now
    guard now - windowStart >= 3 else { return }
    windowStart = now
    defer {
      busy.removeAll(keepingCapacity: true)
      deltas.removeAll(keepingCapacity: true)
    }
    let work = busy.filter { $0 > 0.5 }.sorted()
    guard work.count > 5 else { return }
    let expected = link.duration > 0 ? link.duration * 1000 : 16.67
    let long = deltas.filter { $0 > expected * 1.5 }.count
    NSLog(
      "DBG ntv main busy p50=%.2f p95=%.2f max=%.2f ms passes=%d | frames=%d long=%d maxGap=%.1f ms",
      work[work.count / 2], work[Int(Double(work.count) * 0.95)], work.last ?? 0, work.count, deltas.count, long,
      deltas.max() ?? 0)
  }
}
