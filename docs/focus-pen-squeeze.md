# Xiaomi Focus Pen Pro squeeze

On the tested Xiaomi Pad 7 (Android 16, HyperOS 3.0.304.0), squeezing Focus Pen
Pro sends Android key code 194 with separate DOWN and UP events. UScreen maps
these to its existing primary stylus side button (`BTN_STYLUS`), using pen
actions 5/6. The button stays held until the squeeze is released. No new
protocol messages or desktop shortcuts are introduced.

Only the observed Focus Pen Pro device is accepted: vendor `0x0022`, product
`0x5081`, and a name containing `Focus Pen Pro`. Key code 194 is also Android's
`KEYCODE_BUTTON_7`; unrelated gamepad and keyboard events are not consumed.
Repeated DOWN events do not toggle the button; only the matching key UP
releases the squeeze source.

## Tablet setting

In HyperOS pen settings, set squeeze to **Switch between brush and eraser**.
With **Show shortcut panel**, Xiaomi's panel took focus and prevented delivery
to the Activity. UScreen does not change this preference. The desktop action
is determined by the drawing application's binding for the stylus side button.

No PenEngine SDK, additional AAR, registration or build flag is needed by this
implementation. SDK-free squeeze delivery was verified on the tested Pad 7
firmware, including after a tablet reboot. Other Xiaomi firmware versions
have not been tested.

## Keep holds across hover exits

`PenButtonState` combines MotionEvent buttons and the independent squeeze
key. A hover sample with no MotionEvent button cannot release a squeeze.
Physical proximity exit releases MotionEvent buttons but waits for the squeeze
key's UP. Focus loss, pause and disconnect clear held state; a new connection
never replays an old press.

Keeping raw `BTN_STYLUS=1` alone is insufficient: libinput releases tablet
buttons when the virtual tool leaves proximity. The host therefore clears tip
contact and pressure immediately, but defers virtual proximity-out while the
button is held. Button-up is emitted in its own frame before the deferred
exit. A real hover/contact-down sample cancels that exit if the pen returns.

If squeeze begins just after a hover dropout, the host can restore the last
known tool and position. A button packet's placeholder `(0, 0)` never invents
a first position. Connection/mode cleanup releases the button before the tool
and forgets the retained sample. Pointer handoff waits for the actual virtual
proximity exit.

## Validation

Run `cargo test --release --manifest-path host/Cargo.toml` and, in `android`,
`./gradlew :app:testReleaseUnitTest`. Host event-frame tests cover held exits,
return, tip lift, cleanup ordering, retained position and pointer handoff. JVM
tests cover overlapping sources, repeated events, hover exit and cleanup.

Retained before/after captures on CachyOS/KDE Wayland, Xiaomi Pad 7 and Focus
Pen Pro (Android 16 / HyperOS 3.0.304.0), with PenEngine bundled, exercised two
holds in each case: stay nearby, leave and release away, leave and return
before release. All six post-fix physical DOWN/UP pairs matched app, raw Linux
and libinput button pairs, without early release. No tip contacts or dropped
input events were recorded. These are prior captures of the implementation,
not a fresh hardware run of this PR branch. They do not cover Samsung hardware
or live focus-loss/reconnect behavior; cleanup is covered by state tests.

A later SDK-free build of the same key-handling path was tested before and
after a verified tablet reboot. In the selected post-reboot recording, all
four physical squeeze pairs matched app messages, raw Linux `BTN_STYLUS` and
libinput pairs, without early release, tip contact or dropped input events.
All recorder streams were retained in full and the Activity/PID stayed the
same before and after the phase. The APK had no PenEngine class definitions;
a fresh-process startup confirmed that the SDK was absent. This establishes
SDK-free delivery on that device/firmware. It is not a fresh hardware run of
the final PR APK, nor a new SDK-free test of intentional proximity exits.
