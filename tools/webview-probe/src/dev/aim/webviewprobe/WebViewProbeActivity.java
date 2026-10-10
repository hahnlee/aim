package dev.aim.webviewprobe;

import android.app.Activity;
import android.os.Bundle;
import android.util.Log;
import android.webkit.RenderProcessGoneDetail;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.LinearLayout;
import android.widget.TextView;
import org.json.JSONTokener;

/** Normal app consumer of the installed original Android WebView provider. */
public final class WebViewProbeActivity extends Activity {
    private static final String TAG = "AIMWebViewProbe";
    private static final String MARKER = "AIM WEBVIEW ORIGINAL PROVIDER";
    private WebView webView;
    private TextView status;
    private boolean pageFailed;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        Log.i(TAG, "START appPid=" + android.os.Process.myPid());
        var layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        status = new TextView(this);
        status.setText("Loading original Android WebView...");
        status.setTextSize(18);
        layout.addView(status, new LinearLayout.LayoutParams(-1, -2));
        setContentView(layout);
        try {
            webView = new WebView(this);
            layout.addView(webView, new LinearLayout.LayoutParams(-1, 0, 1));
            var provider = WebView.getCurrentWebViewPackage();
            if (provider == null) throw new IllegalStateException("original WebView provider unavailable");
            String providerDescription = provider.packageName + " version=" + provider.versionName
                    + " code=" + provider.getLongVersionCode();
            Log.i(TAG, "PROVIDER " + providerDescription);
            webView.getSettings().setJavaScriptEnabled(true);
            webView.setWebViewClient(new WebViewClient() {
                @Override public void onPageFinished(WebView view, String url) {
                    Log.i(TAG, "PAGE_FINISHED url=" + url);
                    if (android.os.Build.VERSION.SDK_INT >= 29)
                        Log.i(TAG, "RENDERER handle=" + view.getWebViewRenderProcess());
                    view.evaluateJavascript("document.getElementById('aim-marker').textContent", result -> {
                        try {
                            Object value = new JSONTokener(result).nextValue();
                            if (pageFailed || !MARKER.equals(value))
                                throw new IllegalStateException("DOM marker mismatch: " + result);
                            status.setText("DOM PASS — " + providerDescription);
                            Log.i(TAG, "DOM_PASS marker=" + value + " provider=" + providerDescription);
                        } catch (Exception failure) {
                            pageFailed = true;
                            status.setText("DOM FAIL: " + failure);
                            Log.e(TAG, "DOM_FAIL", failure);
                        }
                    });
                }
                @Override public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError error) {
                    if (request.isForMainFrame()) {
                        pageFailed = true;
                        status.setText("PAGE FAIL: " + error.getDescription());
                        Log.e(TAG, "PAGE_FAIL code=" + error.getErrorCode() + " description=" + error.getDescription());
                    }
                }
                @Override public boolean onRenderProcessGone(WebView view, RenderProcessGoneDetail detail) {
                    pageFailed = true;
                    status.setText("RENDERER FAIL");
                    Log.e(TAG, "RENDERER_GONE crashed=" + detail.didCrash() + " priority=" + detail.rendererPriorityAtExit());
                    view.destroy();
                    webView = null;
                    return true;
                }
            });
            webView.loadDataWithBaseURL("https://aim.invalid/probe/", "<!doctype html><html><head>"
                    + "<meta name='viewport' content='width=device-width,initial-scale=1'>"
                    + "<style>body{margin:0;background:#123c2d;color:#fff;font:24px sans-serif}"
                    + "main{padding:32px}h1{color:#85efac;font-size:34px}.box{background:#216347;padding:24px;border-radius:12px}</style>"
                    + "</head><body><main><h1>Original Android WebView</h1><div class='box' id='aim-marker'>"
                    + MARKER + "</div><p>Local HTML · JavaScript DOM verification · Original provider</p>"
                    + "<svg width='240' height='100'><rect x='4' y='4' width='232' height='92' rx='12' fill='#85efac'/>"
                    + "<circle cx='120' cy='50' r='30' fill='#123c2d'/></svg></main></body></html>",
                    "text/html", "UTF-8", null);
        } catch (RuntimeException | Error failure) {
            Log.e(TAG, "PROVIDER_INIT_FAIL", failure);
            throw failure;
        }
    }
    @Override protected void onDestroy() {
        if (webView != null) webView.destroy();
        super.onDestroy();
    }
}
