package com.uscreen;

import android.media.MediaCodec;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.net.Socket;

/** Exercise the real render thread and MediaCodec error path on Android. */
public final class DecoderFailureCheck {
    private static Field field(String name) throws Exception {
        Field field = VideoReceiver.class.getDeclaredField(name);
        field.setAccessible(true);
        return field;
    }

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static void outputFailure(boolean stale, boolean stopping) throws Exception {
        VideoReceiver receiver = new VideoReceiver();
        MediaCodec old = MediaCodec.createDecoderByType("video/avc");
        MediaCodec current = stale ? MediaCodec.createDecoderByType("video/avc") : old;
        Socket socket = new Socket();
        try {
            field("mediaCodec").set(receiver, current);
            field("socket").set(receiver, socket);
            field("codecAlive").set(receiver, !stopping);
            Method start = VideoReceiver.class.getDeclaredMethod("startOutputThread", MediaCodec.class);
            start.setAccessible(true);
            // A real, unstarted codec rejects dequeueOutputBuffer. This reaches
            // the same render-thread exception path as a native hardware refusal.
            start.invoke(receiver, old);
            Thread thread = (Thread) field("outputThread").get(receiver);
            thread.join(3000);
            check(!thread.isAlive(), "Failed render thread did not terminate");
            check(socket.isClosed() == (!stale && !stopping),
                    "A current decoder failure must wake the receiver; stale/stop failures must not close its socket");
            check(field("mediaCodec").get(receiver) == current,
                    "Render thread must leave teardown to the receive coroutine");
        } finally {
            field("codecAlive").set(receiver, false);
            socket.close();
            old.release();
            if (current != old) current.release();
        }
    }

    public static void main(String[] args) throws Exception {
        try {
            outputFailure(false, false);
            outputFailure(true, false);
            outputFailure(false, true);
            System.out.println("PASS: decoder failure wakes receiver; stale and stopped render threads preserve its connection");
        } catch (Throwable failure) {
            failure.printStackTrace();
            System.exit(1);
        }
    }
}
