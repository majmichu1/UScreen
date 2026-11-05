package com.uscreen;

import java.lang.reflect.Field;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import kotlinx.coroutines.Job;
import okhttp3.Protocol;
import okhttp3.Request;
import okhttp3.Response;
import okhttp3.WebSocket;
import okhttp3.WebSocketListener;
import okio.ByteString;
import org.json.JSONObject;

/** Standalone Android-runtime checks; no test framework or app installation needed. */
public final class PendingConfigCheck {
    private static final Request REQUEST = new Request.Builder().url("http://localhost/").build();
    private static final Response RESPONSE = new Response.Builder().request(REQUEST)
            .protocol(Protocol.HTTP_1_1).code(101).message("Switching Protocols").build();

    private static final class Socket implements WebSocket {
        final List<JSONObject> configs = new CopyOnWriteArrayList<>();
        final List<String> types = new CopyOnWriteArrayList<>();
        boolean acceptConfig = true;
        int rejectedBitrate = -1;
        volatile Runnable beforeConfigSend;
        volatile Runnable beforeAuthSend;

        @Override public Request request() { return REQUEST; }
        @Override public long queueSize() { return 0; }
        @Override public boolean send(String text) {
            try {
                JSONObject message = new JSONObject(text);
                if ("auth".equals(message.optString("type")) && beforeAuthSend != null) {
                    beforeAuthSend.run();
                }
                if ("config".equals(message.optString("type"))) {
                    Runnable hook = beforeConfigSend;
                    beforeConfigSend = null;
                    if (hook != null) hook.run();
                    if (!acceptConfig || message.getInt("bitrate") == rejectedBitrate) return false;
                    configs.add(message);
                }
                types.add(message.optString("type"));
                return true;
            } catch (Exception e) { throw new AssertionError(e); }
        }
        @Override public boolean send(ByteString bytes) { throw new AssertionError("Unexpected binary message"); }
        @Override public boolean close(int code, String reason) { return true; }
        @Override public void cancel() {}
    }

    private static Field field(String name) throws Exception {
        Field field = TouchCapture.class.getDeclaredField(name);
        field.setAccessible(true);
        return field;
    }

    private static WebSocketListener listener(TouchCapture capture) throws Exception {
        return (WebSocketListener) field("wsListener").get(capture);
    }

    private static Socket open(TouchCapture capture) throws Exception {
        Socket socket = new Socket();
        open(capture, socket);
        return socket;
    }

    private static void open(TouchCapture capture, Socket socket) throws Exception {
        field("webSocket").set(capture, socket);
        capture.setToken("test-token");
        listener(capture).onOpen(socket, RESPONSE);
    }

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static void awaitSendBoundary(Thread sender) throws InterruptedException {
        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(10);
        while (sender.isAlive() && sender.getState() != Thread.State.BLOCKED) {
            check(System.nanoTime() < deadline, "New change did not reach its send boundary");
            Thread.sleep(1);
        }
    }

    private static void sentConfigMustNotOverrideNewHostSettings() throws Exception {
        TouchCapture capture = new TouchCapture();
        Socket first = open(capture);
        capture.sendConfig(15000, 60);
        check(first.configs.size() == 1 && first.configs.get(0).getInt("fps") == 60,
                "The user's change was not sent");
        listener(capture).onMessage(first, "{\"fps\":60,\"bitrate_kbps\":40000}");
        capture.disconnect();
        Socket next = open(capture);
        check(next.configs.isEmpty(), "Reconnect replayed the old 15000 kbps over the host's 40000 kbps");
        capture.disconnect();
    }

    private static void offlineChangeMustBeSentOnlyOnce() throws Exception {
        TouchCapture capture = new TouchCapture();
        capture.sendConfig(10000, 30);
        capture.sendConfig(25000, 60);
        Socket first = open(capture);
        check(first.configs.size() == 1 && first.configs.get(0).getInt("bitrate") == 25000
                        && first.configs.get(0).getInt("fps") == 60,
                "Reconnect did not send the latest offline change");
        capture.disconnect();
        check(open(capture).configs.isEmpty(), "The delivered offline change was replayed");
        capture.disconnect();
    }

    private static void rejectedChangeMustRemainPending() throws Exception {
        TouchCapture capture = new TouchCapture();
        Socket first = open(capture);
        first.acceptConfig = false;
        capture.sendConfig(30000, 30);
        capture.disconnect();
        Socket next = open(capture);
        check(next.configs.size() == 1 && next.configs.get(0).getInt("bitrate") == 30000
                        && next.configs.get(0).getInt("fps") == 30,
                "A rejected send lost the user's change");
        capture.disconnect();
        check(open(capture).configs.isEmpty(), "The retry was replayed after delivery");
        capture.disconnect();
    }

    private static void concurrentChangeMustNotBeClearedByAnOlderSend() throws Exception {
        TouchCapture capture = new TouchCapture();
        capture.sendConfig(15000, 60);
        Socket first = new Socket();
        first.rejectedBitrate = 25000;
        CountDownLatch entered = new CountDownLatch(1);
        CountDownLatch release = new CountDownLatch(1);
        AtomicReference<Throwable> failure = new AtomicReference<>();
        first.beforeConfigSend = () -> {
            entered.countDown();
            try { check(release.await(10, TimeUnit.SECONDS), "Blocked send was not released"); }
            catch (InterruptedException e) { throw new AssertionError(e); }
        };
        Thread reconnect = new Thread(() -> {
            try { open(capture, first); }
            catch (Throwable e) { failure.set(e); }
        });
        Thread change = new Thread(() -> {
            try { capture.sendConfig(25000, 30); }
            catch (Throwable e) { failure.set(e); }
        });
        reconnect.start();
        try {
            check(entered.await(10, TimeUnit.SECONDS), "Reconnect did not flush the offline change");
            change.start();
            awaitSendBoundary(change);
        } finally { release.countDown(); }
        reconnect.join(10000);
        change.join(10000);
        check(!reconnect.isAlive() && !change.isAlive(), "Configuration sends deadlocked");
        if (failure.get() != null) throw new AssertionError(failure.get());
        capture.disconnect();
        Socket next = open(capture);
        check(next.configs.size() == 1 && next.configs.get(0).getInt("bitrate") == 25000
                        && next.configs.get(0).getInt("fps") == 30,
                "Finishing an older send erased the newer rejected change");
        capture.disconnect();
        check(open(capture).configs.isEmpty(), "The concurrent change was replayed after delivery");
        capture.disconnect();
    }

    private static void pendingChangeMustFollowAuthentication() throws Exception {
        TouchCapture capture = new TouchCapture();
        capture.setNativeResolution(1536, 960, 172, 108);
        Socket socket = new Socket();
        CountDownLatch entered = new CountDownLatch(1);
        CountDownLatch release = new CountDownLatch(1);
        AtomicReference<Throwable> failure = new AtomicReference<>();
        socket.beforeAuthSend = () -> {
            entered.countDown();
            try { check(release.await(10, TimeUnit.SECONDS), "Authentication was not released"); }
            catch (InterruptedException e) { throw new AssertionError(e); }
        };
        Thread reconnect = new Thread(() -> {
            try { open(capture, socket); }
            catch (Throwable e) { failure.set(e); }
        });
        Thread change = new Thread(() -> {
            try { capture.sendConfig(32000, 60); }
            catch (Throwable e) { failure.set(e); }
        });
        reconnect.start();
        try {
            check(entered.await(10, TimeUnit.SECONDS), "Authentication did not start");
            change.start();
            awaitSendBoundary(change);
            check(socket.configs.isEmpty(), "Config was sent before authentication");
        } finally { release.countDown(); }
        reconnect.join(10000);
        change.join(10000);
        check(!reconnect.isAlive() && !change.isAlive(), "Authentication/config sends deadlocked");
        if (failure.get() != null) throw new AssertionError(failure.get());
        check(socket.types.toString().equals("[auth, resolution, config]"),
                "Expected authentication, resolution, then pending config; got " + socket.types);
        capture.disconnect();
    }

    private static void failedAuthenticationMustRemainDisconnected() throws Exception {
        TouchCapture capture = new TouchCapture();
        capture.sendConfig(30000, 30);
        Socket socket = new Socket();
        socket.beforeAuthSend = () -> {
            try {
                listener(capture).onFailure(socket, new java.io.IOException("synthetic auth failure"), null);
                Job retry = (Job) field("reconnectJob").get(capture);
                if (retry != null) retry.cancel(null);
                socket.acceptConfig = false;
            } catch (Exception e) { throw new AssertionError(e); }
        };
        try {
            open(capture, socket);
            check(!capture.isControlConnected(), "onOpen revived a failed control connection");
        } finally { capture.disconnect(); }
        Socket retry = open(capture);
        check(retry.configs.size() == 1 && retry.configs.get(0).getInt("bitrate") == 30000
                        && retry.configs.get(0).getInt("fps") == 30,
                "The failed connection lost the unsent change");
        capture.disconnect();
    }

    public static void main(String[] args) throws Exception {
        try {
            failedAuthenticationMustRemainDisconnected();
            sentConfigMustNotOverrideNewHostSettings();
            offlineChangeMustBeSentOnlyOnce();
            rejectedChangeMustRemainPending();
            concurrentChangeMustNotBeClearedByAnOlderSend();
            pendingChangeMustFollowAuthentication();
            System.out.println("PASS: no replay; offline/latest/rejected/concurrent changes and authentication order");
        } catch (Throwable failure) {
            failure.printStackTrace();
            System.exit(1);
        }
    }
}
