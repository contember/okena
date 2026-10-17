package com.okena.terminalview

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ScrollModelTest {
  private fun model(first: Long = 0, screenTop: Long = 100, screen: Int = 40, view: Int = 40) =
    ScrollModel().apply { update(first, screenTop, screen, view, 0) }

  @Test
  fun startsAtTheLiveTailAndFollowsOutput() {
    val m = model()
    assertEquals(100.0, m.top, 0.0)
    assertTrue(m.follow)
    m.update(0, 130, 40, 40, 0)
    assertEquals(130.0, m.top, 0.0)
  }

  @Test
  fun scrolledUpStaysOnTheSameAbsoluteLine() {
    val m = model()
    m.scrollBy(-20.5)
    assertFalse(m.follow)
    m.update(0, 160, 40, 40, 0)
    assertEquals(79.5, m.top, 1e-9)
    assertEquals(80.5, m.distanceFromBottom, 1e-9)
  }

  @Test
  fun clampsWhenOldLinesAreDropped() {
    val m = model()
    m.scrollTo(10.0)
    m.update(50, 150, 40, 40, 0)
    assertEquals(50.0, m.top, 0.0)
  }

  @Test
  fun reflowKeepsTheDistanceFromTheBottom() {
    val m = model()
    m.scrollBy(-30.0)
    m.update(0, 140, 20, 20, 1)
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
    m.update(100, 100, 40, 40, 0)
    assertEquals(100.0, m.top, 0.0)
    assertTrue(m.follow)
  }
}
