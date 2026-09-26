package dev.darwinart.simple;

import android.app.Activity;
import android.os.Bundle;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.RadioButton;
import android.widget.SeekBar;
import android.widget.TextView;
import android.widget.ToggleButton;

/** A resource-backed, no-native launcher Activity using Android framework widgets. */
public final class MainActivity extends Activity {
    private static native int nativeAnswer();

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        Thread.currentThread().setContextClassLoader(MainActivity.class.getClassLoader());
        FontBootstrap.install();
        String stage = "LinearLayout";
        try {
            LinearLayout root = new LinearLayout(this);
            root.setOrientation(LinearLayout.VERTICAL);
            root.setPadding(32, 20, 32, 20);
            // Material You light-scheme surface tones.  The platform Material
            // theme still owns typography, state lists, ripples, and widget
            // metrics; these values only keep this no-resource fixture tidy.
            root.setBackgroundColor(0xfffffbfe);

            stage = "TextView";
            TextView title = new TextView(this);
            title.setTag("title");
            int titleId = getResources().getIdentifier(
                    "app_title", "string", getPackageName());
            title.setText(titleId == 0
                    ? "Missing APK resource"
                    : getResources().getString(titleId));
            // The no-native APK deliberately catches the missing-library
            // case, while the JNI APK calls this through JavaVMExt/NativeBridge
            // before Activity.onCreate and must observe the real native value.
            try {
                if (nativeAnswer() != 42) {
                    throw new IllegalStateException("native answer mismatch");
                }
            } catch (UnsatisfiedLinkError absent) {
                // Valid for the no-so APK variant.
            }
            root.addView(title, matchWidth());

            stage = "CheckBox";
            LinearLayout toggles = new LinearLayout(this);
            toggles.setOrientation(LinearLayout.HORIZONTAL);
            CheckBox checkBox = new CheckBox(this);
            checkBox.setTag("checkbox");
            checkBox.setText("CheckBox");
            checkBox.setTextColor(0xff1d1b20);
            checkBox.setChecked(true);
            toggles.addView(checkBox, weighted());

            stage = "RadioButton";
            RadioButton radio = new RadioButton(this);
            radio.setTag("radio");
            radio.setText("RadioButton");
            radio.setTextColor(0xff1d1b20);
            radio.setChecked(true);
            toggles.addView(radio, weighted());
            root.addView(toggles, matchWidth());

            stage = "ToggleButton";
            ToggleButton toggle = new ToggleButton(this);
            toggle.setTag("toggle");
            toggle.setTextOn("Toggle on");
            toggle.setTextOff("Toggle off");
            toggle.setTextColor(0xff1d1b20);
            root.addView(toggle, matchWidth());

            stage = "SeekBar";
            SeekBar seek = new SeekBar(this);
            seek.setTag("seek");
            seek.setMax(100);
            seek.setProgress(42);
            root.addView(seek, matchWidth());

            stage = "ProgressBar";
            ProgressBar progress = new ProgressBar(
                    this, null, android.R.attr.progressBarStyleHorizontal);
            progress.setTag("progress");
            progress.setMax(100);
            progress.setProgress(68);
            root.addView(progress, matchWidth());

            stage = "Button";
            Button button = new Button(this);
            button.setTag("button");
            button.setText("Default Android Button");
            button.setAllCaps(false);
            button.setOnClickListener(view -> button.setText("Button pressed"));
            root.addView(button, matchWidth());

            stage = "setContentView";
            setContentView(root);
        } catch (Throwable error) {
            throw new IllegalStateException("Default widget failed: " + stage, error);
        }
    }

    private static LinearLayout.LayoutParams matchWidth() {
        return new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
    }

    private static LinearLayout.LayoutParams weighted() {
        return new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1);
    }

}
