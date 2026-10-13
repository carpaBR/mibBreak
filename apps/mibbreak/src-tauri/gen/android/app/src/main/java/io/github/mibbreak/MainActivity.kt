package io.github.mibbreak

import android.app.PictureInPictureParams
import android.content.pm.ActivityInfo
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.util.Rational
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  private var webView: WebView? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    window.decorView.setBackgroundColor(Color.parseColor("#101114"))
  }

  override fun onWebViewCreate(webView: WebView) {
    this.webView = webView
    webView.addJavascriptInterface(Bridge(), "MibBreakAndroid")
    // Keep the page clear of the status / navigation bars and the camera cutout
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { v, insets ->
      val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
      if (isInPip()) v.setPadding(0, 0, 0, 0) else v.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      insets
    }
  }

  private fun isInPip(): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && isInPictureInPictureMode

  override fun onPictureInPictureModeChanged(isInPictureInPictureMode: Boolean, newConfig: Configuration) {
    super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
    findViewById<View>(android.R.id.content).requestApplyInsets()
    webView?.evaluateJavascript("window.__onPip && window.__onPip($isInPictureInPictureMode)", null)
  }

  /** Called from the page as window.MibBreakAndroid.* */
  inner class Bridge {
    @JavascriptInterface
    fun pipSupported(): Boolean =
      Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && packageManager.hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)

    /** portrait | landscape | auto — the app decides, not the rotation sensor (except flipping sides). */
    @JavascriptInterface
    fun setOrientation(mode: String) {
      runOnUiThread {
        requestedOrientation = when (mode) {
          "portrait" -> ActivityInfo.SCREEN_ORIENTATION_PORTRAIT
          "landscape" -> ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
          else -> ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
        }
      }
    }

    @JavascriptInterface
    fun enterPip(width: Int, height: Int) {
      if (!pipSupported()) return
      runOnUiThread {
        // Android accepts aspect ratios between 1:2.39 and 2.39:1 only
        val r = (width.coerceAtLeast(1).toFloat() / height.coerceAtLeast(1)).coerceIn(0.42f, 2.38f)
        val ratio = Rational((r * 1000).toInt(), 1000)
        enterPictureInPictureMode(PictureInPictureParams.Builder().setAspectRatio(ratio).build())
      }
    }
  }
}
