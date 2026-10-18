package com.okena.terminalview

import android.app.Dialog
import android.content.Context
import android.graphics.Color
import android.os.SystemClock
import android.text.util.Linkify
import android.util.Log
import android.util.TypedValue
import android.view.Gravity
import android.view.ViewGroup
import android.view.ViewTreeObserver
import android.view.WindowManager
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import java.util.regex.Pattern

/**
 * "Select text" mode: a frozen plain-text copy of the terminal's scrollback and screen in a
 * selectable [TextView], so the user gets the native selection handles, magnifier and floating
 * ActionMode (Copy, Select all, Share). Opened by [TerminalRendererView.openSelectText]; the
 * text comes from `okena_rv_text_range` (soft wraps joined, trailing blanks trimmed). It is a
 * snapshot: output that arrives while it is open is not shown.
 */
internal class TerminalTextDialog(
  context: Context,
  private val content: String,
  /** Newlines before the line the terminal showed at its top edge. */
  private val anchorNewlines: Int,
  fontSizeDp: Float,
  background: Int,
  foreground: Int,
  fonts: TerminalFonts,
) : Dialog(context, android.R.style.Theme_Black_NoTitleBar_Fullscreen) {

  private val scroll = ScrollView(context)
  private val body = TextView(context)

  init {
    val started = SystemClock.elapsedRealtime()
    val density = context.resources.displayMetrics.density
    val root = LinearLayout(context).apply {
      orientation = LinearLayout.VERTICAL
      setBackgroundColor(background)
      fitsSystemWindows = true
    }

    val bar = FrameLayout(context).apply {
      layoutParams = LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, (52 * density).toInt())
    }
    bar.addView(
      TextView(context).apply {
        setText("Select text")
        setTextColor(foreground)
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 17f)
        paint.isFakeBoldText = true
      },
      FrameLayout.LayoutParams(
        ViewGroup.LayoutParams.WRAP_CONTENT,
        ViewGroup.LayoutParams.WRAP_CONTENT,
        Gravity.CENTER,
      ),
    )
    bar.addView(
      Button(context, null, android.R.attr.borderlessButtonStyle).apply {
        setText("Close")
        isAllCaps = false
        setTextColor(foreground)
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f)
        contentDescription = "Close select text"
        setOnClickListener { dismiss() }
      },
      FrameLayout.LayoutParams(
        ViewGroup.LayoutParams.WRAP_CONTENT,
        ViewGroup.LayoutParams.WRAP_CONTENT,
        Gravity.END or Gravity.CENTER_VERTICAL,
      ),
    )
    root.addView(bar)
    root.addView(
      android.view.View(context).apply { setBackgroundColor(0x1fffffff) },
      LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, maxOf(1, (density / 2).toInt())),
    )

    body.apply {
      setText(content)
      setTextColor(foreground)
      setLinkTextColor(0xff8ab4f8.toInt())
      highlightColor = 0x66264f78
      typeface = fonts.primary[0]
      setTextSize(TypedValue.COMPLEX_UNIT_DIP, fontSizeDp)
      setLineSpacing(0f, 1.1f)
      val pad = (8 * density).toInt()
      setPadding(pad, pad, pad, (24 * density).toInt())
      setTextIsSelectable(true)
      // Only explicit http(s) links: bare-domain, phone and e-mail detection turns
      // file names such as `main.rs` into links.
      Linkify.addLinks(this, URL_PATTERN, null)
      contentDescription = "Terminal text"
    }
    scroll.isFillViewport = true
    scroll.addView(body, ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
    root.addView(scroll, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))
    setContentView(root)
    window?.setLayout(WindowManager.LayoutParams.MATCH_PARENT, WindowManager.LayoutParams.MATCH_PARENT)
    window?.setBackgroundDrawable(android.graphics.drawable.ColorDrawable(Color.BLACK))

    // Scroll to the terminal's viewport once the text is laid out (first pre-draw).
    body.viewTreeObserver.addOnPreDrawListener(
      object : ViewTreeObserver.OnPreDrawListener {
        override fun onPreDraw(): Boolean {
          val layout = body.layout ?: return true
          body.viewTreeObserver.removeOnPreDrawListener(this)
          val offset = lineStartOffset(content, anchorNewlines)
          scroll.scrollTo(0, layout.getLineTop(layout.getLineForOffset(offset)) + body.paddingTop)
          Log.i(
            TAG,
            "select text: ${content.length} chars, ${layout.lineCount} lines laid out " +
              "${SystemClock.elapsedRealtime() - started} ms after open",
          )
          return true
        }
      },
    )
  }

  private companion object {
    const val TAG = "OkenaTerminalView"
    val URL_PATTERN: Pattern = Pattern.compile("https?://[^\\s<>\"'`]+[^\\s<>\"'`.,:;!?)\\]]")

    /** Char offset of the start of line [line] (0-based). */
    fun lineStartOffset(text: String, line: Int): Int {
      if (line <= 0) return 0
      var seen = 0
      for (i in text.indices) {
        if (text[i] == '\n' && ++seen == line) return i + 1
      }
      return text.length
    }
  }
}
