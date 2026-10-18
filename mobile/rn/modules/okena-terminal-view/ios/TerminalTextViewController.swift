import UIKit

/// "Select text" mode: a frozen plain-text copy of the terminal's scrollback
/// and screen in a system text view, so the user gets the native selection
/// handles, loupe and edit menu (Copy, Select All, Share); http(s) URLs are
/// tappable links.
///
/// Opened by `TerminalRendererView.openSelectText()`; the text comes from
/// `okena_rv_text_range` (soft wraps joined, trailing blanks trimmed). It is a
/// snapshot: output that arrives while it is open is not shown.
final class TerminalTextViewController: UIViewController {
  private let text: String
  /// Newlines before the line the terminal showed at its top edge.
  private let anchorNewlines: Int
  private let fontSize: CGFloat
  private let background: UIColor
  private let foreground: UIColor
  private let textView = UITextView()
  private var scrolledToAnchor = false

  init(text: String, anchorNewlines: Int, fontSize: CGFloat, background: UIColor, foreground: UIColor) {
    self.text = text
    self.anchorNewlines = anchorNewlines
    self.fontSize = fontSize
    self.background = background
    self.foreground = foreground
    super.init(nibName: nil, bundle: nil)
    modalPresentationStyle = .fullScreen
    overrideUserInterfaceStyle = .dark
  }

  required init?(coder: NSCoder) {
    fatalError("init(coder:) is not supported")
  }

  override var preferredStatusBarStyle: UIStatusBarStyle { .lightContent }

  override func viewDidLoad() {
    super.viewDidLoad()
    view.backgroundColor = background

    let bar = UIView()
    bar.translatesAutoresizingMaskIntoConstraints = false
    view.addSubview(bar)

    let title = UILabel()
    title.text = "Select text"
    title.textColor = foreground
    title.font = .systemFont(ofSize: 17, weight: .semibold)
    title.translatesAutoresizingMaskIntoConstraints = false
    bar.addSubview(title)

    let close = UIButton(type: .system)
    close.setTitle("Close", for: .normal)
    close.titleLabel?.font = .systemFont(ofSize: 17, weight: .regular)
    close.tintColor = foreground
    close.accessibilityIdentifier = "select-text-close"
    close.addTarget(self, action: #selector(closeTapped), for: .touchUpInside)
    close.translatesAutoresizingMaskIntoConstraints = false
    bar.addSubview(close)

    let separator = UIView()
    separator.backgroundColor = UIColor(white: 1, alpha: 0.12)
    separator.translatesAutoresizingMaskIntoConstraints = false
    view.addSubview(separator)

    textView.translatesAutoresizingMaskIntoConstraints = false
    textView.isEditable = false
    textView.isSelectable = true
    textView.backgroundColor = background
    let linkColor = UIColor(red: 0.54, green: 0.71, blue: 0.98, alpha: 1)
    textView.tintColor = linkColor
    textView.linkTextAttributes = [.foregroundColor: linkColor, .underlineStyle: NSUnderlineStyle.single.rawValue]
    textView.textContainerInset = UIEdgeInsets(top: 8, left: 6, bottom: 24, right: 6)
    textView.alwaysBounceVertical = true
    textView.layoutManager.allowsNonContiguousLayout = true
    textView.accessibilityIdentifier = "select-text-view"
    textView.attributedText = attributedText()
    view.addSubview(textView)

    let guide = view.safeAreaLayoutGuide
    NSLayoutConstraint.activate([
      bar.topAnchor.constraint(equalTo: guide.topAnchor),
      bar.leadingAnchor.constraint(equalTo: guide.leadingAnchor),
      bar.trailingAnchor.constraint(equalTo: guide.trailingAnchor),
      bar.heightAnchor.constraint(equalToConstant: 44),
      title.centerXAnchor.constraint(equalTo: bar.centerXAnchor),
      title.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
      close.trailingAnchor.constraint(equalTo: bar.trailingAnchor, constant: -16),
      close.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
      separator.topAnchor.constraint(equalTo: bar.bottomAnchor),
      separator.leadingAnchor.constraint(equalTo: view.leadingAnchor),
      separator.trailingAnchor.constraint(equalTo: view.trailingAnchor),
      separator.heightAnchor.constraint(equalToConstant: 1 / UIScreen.main.scale),
      textView.topAnchor.constraint(equalTo: separator.bottomAnchor),
      textView.leadingAnchor.constraint(equalTo: guide.leadingAnchor),
      textView.trailingAnchor.constraint(equalTo: guide.trailingAnchor),
      textView.bottomAnchor.constraint(equalTo: view.bottomAnchor),
    ])
  }

  /// The text in the terminal font, with explicit http(s) URLs as links. Not
  /// `dataDetectorTypes`: it also links bare domains, and file names such as
  /// `main.rs` look like domains.
  private func attributedText() -> NSAttributedString {
    let font = UIFont(name: "JetBrainsMono-Regular", size: fontSize)
      ?? .monospacedSystemFont(ofSize: fontSize, weight: .regular)
    let result = NSMutableAttributedString(
      string: text, attributes: [.font: font, .foregroundColor: foreground])
    let whole = NSRange(location: 0, length: result.length)
    for match in Self.urlPattern?.matches(in: text, range: whole) ?? [] {
      let url = (text as NSString).substring(with: match.range)
      if let link = URL(string: url) { result.addAttribute(.link, value: link, range: match.range) }
    }
    return result
  }

  /// Same rule as the Android dialog: stop before whitespace, quotes and
  /// brackets, and drop trailing punctuation.
  private static let urlPattern = try? NSRegularExpression(
    pattern: "https?://[^\\s<>\"'`]+[^\\s<>\"'`.,:;!?)\\]]")

  override func viewDidLayoutSubviews() {
    super.viewDidLayoutSubviews()
    guard !scrolledToAnchor, textView.bounds.height > 0 else { return }
    scrolledToAnchor = true
    scrollToAnchor()
  }

  /// Put the line the terminal showed at its top at the top of the text view.
  private func scrollToAnchor() {
    let start = CACurrentMediaTime()
    let offset = Self.utf16Offset(ofLine: anchorNewlines, in: text)
    let layout = textView.layoutManager
    // Lay out everything once: the content height (and so the clamp below)
    // is only an estimate until then.
    layout.ensureLayout(for: textView.textContainer)
    textView.layoutIfNeeded()
    let glyph = layout.glyphIndexForCharacter(at: offset)
    let rect = layout.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
    let maxY = max(textView.contentSize.height - textView.bounds.height, 0)
    textView.setContentOffset(CGPoint(x: 0, y: min(rect.minY, maxY)), animated: false)
    if UserDefaults.standard.bool(forKey: "OkenaTerminalPerfLog") {
      NSLog("DBG ntv select text layout: %.1f ms", (CACurrentMediaTime() - start) * 1000)
    }
  }

  /// UTF-16 offset of the start of line `line` (0-based).
  static func utf16Offset(ofLine line: Int, in text: String) -> Int {
    guard line > 0 else { return 0 }
    var seen = 0
    var offset = 0
    for unit in text.utf16 {
      offset += 1
      if unit == 0x0A {
        seen += 1
        if seen == line { return offset }
      }
    }
    return offset
  }

  @objc private func closeTapped() {
    dismiss(animated: true)
  }
}
