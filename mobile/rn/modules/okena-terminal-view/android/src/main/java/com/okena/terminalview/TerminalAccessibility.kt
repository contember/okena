package com.okena.terminalview

import android.graphics.Rect
import android.os.Bundle
import android.view.View
import androidx.core.view.accessibility.AccessibilityNodeInfoCompat
import androidx.customview.widget.ExploreByTouchHelper

/**
 * Basic TalkBack support: one virtual node per visible terminal row, whose text is the row's
 * text. Row texts are collected by [TerminalRendererView] only while touch exploration is on.
 */
internal class TerminalAccessibility(
  host: View,
  private val rowCount: () -> Int,
  private val rowText: (Int) -> String?,
  private val rowBounds: (Int) -> Rect?,
  private val rowAt: (Float) -> Int,
) : ExploreByTouchHelper(host) {

  override fun getVirtualViewAt(x: Float, y: Float): Int {
    val r = rowAt(y)
    return if (r in 0 until rowCount() && rowText(r) != null) r else INVALID_ID
  }

  override fun getVisibleVirtualViews(virtualViewIds: MutableList<Int>) {
    for (r in 0 until rowCount()) {
      if (rowText(r) != null) virtualViewIds.add(r)
    }
  }

  override fun onPopulateNodeForVirtualView(virtualViewId: Int, node: AccessibilityNodeInfoCompat) {
    val text = rowText(virtualViewId) ?: ""
    node.text = text
    node.contentDescription = text.ifEmpty { " " }
    @Suppress("DEPRECATION")
    node.setBoundsInParent(rowBounds(virtualViewId) ?: Rect(0, 0, 1, 1))
  }

  override fun onPerformActionForVirtualView(virtualViewId: Int, action: Int, arguments: Bundle?): Boolean =
    false
}
