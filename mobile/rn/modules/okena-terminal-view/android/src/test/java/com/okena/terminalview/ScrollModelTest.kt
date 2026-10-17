package com.okena.terminalview

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ScrollModelTest {
  private fun model(first: Long = 0, screenTop: Long = 100, screen: Int = 40, view: Int = 40) =
    ScrollModel().apply { update(first, screenTop, screen, view, 0, screenTop + screen - 1) }

  @Test
  fun startsAtTheLiveTailAndFollowsOutput() {
    val m = model()
    assertEquals(100.0, m.top, 0.0)
    assertTrue(m.follow)
    m.update(0, 130, 40, 40, 0, 130 + 40 - 1)
    assertEquals(130.0, m.top, 0.0)
  }

  @Test
  fun scrolledUpStaysOnTheSameAbsoluteLine() {
    val m = model()
    m.scrollBy(-20.5)
    assertFalse(m.follow)
    m.update(0, 160, 40, 40, 0, 160 + 40 - 1)
    assertEquals(79.5, m.top, 1e-9)
    assertEquals(80.5, m.distanceFromBottom, 1e-9)
  }

  @Test
  fun clampsWhenOldLinesAreDropped() {
    val m = model()
    m.scrollTo(10.0)
    m.update(50, 150, 40, 40, 0, 150 + 40 - 1)
    assertEquals(50.0, m.top, 0.0)
  }

  @Test
  fun reflowKeepsTheDistanceFromTheBottom() {
    val m = model()
    m.scrollBy(-30.0)
    m.update(0, 140, 20, 20, 1, 140 + 20 - 1)
    assertEquals(110.0, m.top, 0.0)
  }

  @Test
  fun scrollingBackToTheTailFollowsAgainAndReportsOverscroll() {
    val m = model()
    m.scrollBy(-5.0)
    assertEquals(3.0, m.scrollBy(8.0), 1e-9)
    assertTrue(m.follow)
    assertEquals(-100.0, m.scrollBy(-200.0), 1e-9)
  }

  @Test
  fun aGridTallerThanTheViewShowsItsBottomAtTheTail() {
    val m = model(screen = 41, view = 19)
    assertEquals(122.0, m.top, 0.0)
    assertEquals(0L, m.minTop)
  }

  @Test
  fun altScreenWithoutHistoryPinsToTheScreen() {
    val m = model()
    m.scrollBy(-50.0)
    m.update(100, 100, 40, 40, 0, 100 + 40 - 1)
    assertEquals(100.0, m.top, 0.0)
    assertTrue(m.follow)
  }

  // A grid taller than the view (our resize denied, or the keyboard just opened) with a short
  // shell prompt at its top: the live tail must show the prompt, not the blank bottom rows.
  @Test
  fun aTallGridWithTheCursorNearItsTopShowsTheScreenTop() {
    val m = ScrollModel()
    m.update(0, 0, 36, 34, 0, 6)
    assertEquals(0.0, m.top, 0.0)
    assertTrue(m.follow)
    // The keyboard opens: the view shrinks to 16 rows, the grid stays 36 rows tall.
    m.update(0, 0, 36, 16, 1, 6)
    assertEquals(0.0, m.top, 0.0)
    assertTrue(m.follow)
    assertEquals(20L, m.maxTop)
    // ... and closes again.
    m.update(0, 0, 36, 34, 2, 6)
    assertEquals(0.0, m.top, 0.0)
  }

  @Test
  fun theLiveTailFollowsTheCursorDownATallGrid() {
    val m = ScrollModel()
    m.update(0, 0, 36, 16, 0, 15)
    assertEquals(0.0, m.top, 0.0)
    m.update(0, 0, 36, 16, 0, 16)
    assertEquals(1.0, m.top, 0.0)
    m.update(0, 0, 36, 16, 0, 35)
    assertEquals(20.0, m.top, 0.0)
    // Output past the screen bottom scrolls the screen; the cursor stays on its last row.
    m.update(0, 10, 36, 16, 0, 45)
    assertEquals(30.0, m.top, 0.0)
  }

  @Test
  fun theRowsBelowATallGridsLiveTailStayReachable() {
    val m = ScrollModel()
    m.update(0, 0, 36, 16, 0, 2)
    assertEquals(0.0, m.scrollBy(5.0), 0.0)
    assertEquals(5.0, m.top, 0.0)
    assertTrue(m.follow)
    // A new frame with the same tail keeps the position below it.
    m.update(0, 0, 36, 16, 0, 2)
    assertEquals(5.0, m.top, 0.0)
    assertEquals(4.0, m.scrollBy(19.0), 1e-9)
    assertEquals(20.0, m.top, 0.0)
    // Back to the live tail, as after input.
    m.toBottom()
    assertEquals(0.0, m.top, 0.0)
  }

  @Test
  fun scrolledUpInATallGridKeepsTheDistanceFromTheBottomOnReflow() {
    val m = ScrollModel()
    m.update(0, 100, 36, 36, 0, 135)
    m.scrollBy(-30.0)
    assertFalse(m.follow)
    m.update(0, 100, 36, 16, 1, 135)
    assertEquals(90.0, m.top, 0.0)
    assertFalse(m.follow)
  }
}
