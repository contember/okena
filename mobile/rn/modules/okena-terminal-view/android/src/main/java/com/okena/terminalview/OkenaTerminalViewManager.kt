package com.okena.terminalview

import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.ReactContext
import com.facebook.react.bridge.ReadableMap
import com.facebook.react.bridge.WritableMap
import com.facebook.react.module.annotations.ReactModule
import com.facebook.react.uimanager.SimpleViewManager
import com.facebook.react.uimanager.ThemedReactContext
import com.facebook.react.uimanager.UIManagerHelper
import com.facebook.react.uimanager.ViewManagerDelegate
import com.facebook.react.uimanager.annotations.ReactProp
import com.facebook.react.viewmanagers.OkenaTerminalViewManagerDelegate
import com.facebook.react.viewmanagers.OkenaTerminalViewManagerInterface

/**
 * Fabric view manager for the `OkenaTerminalView` codegen component.
 *
 * Props arrive through the generated [OkenaTerminalViewManagerDelegate] and are
 * copied onto [TerminalRendererView]; its [TerminalRendererView.Listener]
 * callbacks are dispatched as [TerminalViewEvent]s. Commands also go through the
 * delegate. The contract is the spec in `src/OkenaTerminalViewNativeComponent.ts`.
 */
@ReactModule(name = OkenaTerminalViewManager.NAME)
class OkenaTerminalViewManager :
  SimpleViewManager<TerminalRendererView>(),
  OkenaTerminalViewManagerInterface<TerminalRendererView> {

  private val delegate = OkenaTerminalViewManagerDelegate(this)

  override fun getDelegate(): ViewManagerDelegate<TerminalRendererView> = delegate

  override fun getName(): String = NAME

  override fun createViewInstance(context: ThemedReactContext): TerminalRendererView {
    val view = TerminalRendererView(context)
    view.listener = EventForwarder(context, view)
    return view
  }

  // ── Props ────────────────────────────────────────────────────────────────

  @ReactProp(name = "connId")
  override fun setConnId(view: TerminalRendererView, value: String?) {
    view.connId = value ?: ""
  }

  @ReactProp(name = "terminalId")
  override fun setTerminalId(view: TerminalRendererView, value: String?) {
    view.terminalId = value ?: ""
  }

  @ReactProp(name = "fontSize")
  override fun setFontSize(view: TerminalRendererView, value: Double) {
    view.fontSize = value
  }

  @ReactProp(name = "theme")
  override fun setTheme(view: TerminalRendererView, value: ReadableMap?) {
    if (value == null) return
    argb(value, "background")?.let { view.backgroundArgb = it }
    argb(value, "foreground")?.let { view.foregroundArgb = it }
    argb(value, "cursor")?.let { view.cursorArgb = it }
    argb(value, "selection")?.let { view.selectionArgb = it }
  }

  @ReactProp(name = "focused")
  override fun setFocused(view: TerminalRendererView, value: Boolean) {
    view.focusedPane = value
  }

  // ── Commands ─────────────────────────────────────────────────────────────

  override fun scrollToBottom(view: TerminalRendererView) = view.scrollToBottom()

  override fun clearSelection(view: TerminalRendererView) = view.clearSelection()

  override fun openSelectText(view: TerminalRendererView) = view.openSelectText()

  // ── Events ───────────────────────────────────────────────────────────────

  override fun getExportedCustomDirectEventTypeConstants(): MutableMap<String, Any> {
    val base = super.getExportedCustomDirectEventTypeConstants() ?: mutableMapOf()
    for (name in EVENT_NAMES) {
      base["top$name"] = mapOf("registrationName" to "on$name")
    }
    return base
  }

  private class EventForwarder(
    private val context: ReactContext,
    private val view: TerminalRendererView,
  ) : TerminalRendererView.Listener {
    override fun onTap(col: Int, row: Int) =
      emit("Tap") {
        putInt("col", col)
        putInt("row", row)
      }

    override fun onGridSize(cols: Int, rows: Int, cellWidth: Double, cellHeight: Double) =
      emit("GridSize") {
        putInt("cols", cols)
        putInt("rows", rows)
        putDouble("cellWidth", cellWidth)
        putDouble("cellHeight", cellHeight)
      }

    override fun onFontSizeChange(size: Double) = emit("FontSizeChange") { putDouble("size", size) }

    override fun onSelectionCopied(text: String) = emit("SelectionCopied") { putString("text", text) }

    override fun onScrollStateChange(atBottom: Boolean) =
      emit("ScrollStateChange") { putBoolean("atBottom", atBottom) }

    override fun onOutputPaused(paused: Boolean) =
      emit("OutputPausedChange") { putBoolean("paused", paused) }

    private inline fun emit(name: String, fill: WritableMap.() -> Unit) {
      val dispatcher = UIManagerHelper.getEventDispatcherForReactTag(context, view.id) ?: return
      val payload = Arguments.createMap().apply(fill)
      dispatcher.dispatchEvent(
        TerminalViewEvent(UIManagerHelper.getSurfaceId(context), view.id, "top$name", payload),
      )
    }
  }

  companion object {
    const val NAME = "OkenaTerminalView"

    private val EVENT_NAMES =
      listOf(
        "Tap",
        "GridSize",
        "FontSizeChange",
        "SelectionCopied",
        "ScrollStateChange",
        "OutputPausedChange",
      )

    /** Packed unsigned ARGB sent as a JS number → Android color int. */
    private fun argb(map: ReadableMap, key: String): Int? =
      if (map.hasKey(key) && !map.isNull(key)) map.getDouble(key).toLong().toInt() else null
  }
}
