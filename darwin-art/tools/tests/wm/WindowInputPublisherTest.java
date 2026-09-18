package dev.darwinart.runtime.wm;
import android.graphics.Rect;
import android.view.View;
public final class WindowInputPublisherTest {
 public static void main(String[] args) {
  Rect valid=new Rect(0,0,720,1280);
  if (!WindowInputPublisher.inputVisible(valid,View.VISIBLE)) throw new AssertionError();
  if (WindowInputPublisher.inputVisible(valid,View.INVISIBLE)) throw new AssertionError();
  if (WindowInputPublisher.inputVisible(valid,View.GONE)) throw new AssertionError();
  if (WindowInputPublisher.inputVisible(null,View.VISIBLE)) throw new AssertionError();
  if (WindowInputPublisher.inputVisible(new Rect(0,0,0,1280),View.VISIBLE)) throw new AssertionError();
  if (WindowInputPublisher.decodeStatus(0) != WindowFocusPublicationDelivery.Result.ACCEPTED) throw new AssertionError();
  if (WindowInputPublisher.decodeStatus(1) != WindowFocusPublicationDelivery.Result.BACKPRESSURED) throw new AssertionError();
  if (WindowInputPublisher.decodeStatus(2) != WindowFocusPublicationDelivery.Result.TERMINAL) throw new AssertionError();
  for (int invalid : new int[] {-1, 3, Integer.MAX_VALUE}) {
   try { WindowInputPublisher.decodeStatus(invalid); throw new AssertionError(); }
   catch (IllegalStateException expected) {}
  }
  if (WindowInputPublisher.publish(null, valid, View.VISIBLE) != WindowFocusPublicationDelivery.Result.TERMINAL) throw new AssertionError();
  if (WindowInputPublisher.remove(null) != WindowFocusPublicationDelivery.Result.TERMINAL) throw new AssertionError();
  try { WindowInputPublisher.send(null); throw new AssertionError(); }
  catch (IllegalArgumentException expected) {}
  System.out.println("WMS actual view visibility and frame eligibility: PASS");
 }
}
