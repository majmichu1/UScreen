# OkHttp
-dontwarn okhttp3.**
-dontwarn okio.**
-keep class okhttp3.** { *; }
-keep class okio.** { *; }

# Keep WebSocket listener
-keep class com.uscreen.TouchCapture$* { *; }

# JSON
-keep class org.json.** { *; }

# Keep all classes in our package
-keep class com.uscreen.** { *; }
