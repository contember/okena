package com.okena.terminalview

import com.facebook.react.bridge.WritableMap
import com.facebook.react.uimanager.events.Event

/**
 * One event from [TerminalRendererView] to JS. [name] is the `top*` name
 * registered in [OkenaTerminalViewManager.getExportedCustomDirectEventTypeConstants];
 * Fabric maps it to the codegen event (`topTap` → `onTap`).
 */
internal class TerminalViewEvent(
  surfaceId: Int,
  viewTag: Int,
  private val name: String,
  private val payload: WritableMap,
) : Event<TerminalViewEvent>(surfaceId, viewTag) {
  override fun getEventName(): String = name

  override fun getEventData(): WritableMap = payload
}
