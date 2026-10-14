import AVFoundation
import CallKit
import Flutter
import PushKit
import UIKit
import UserNotifications
import WebRTC
import flutter_webrtc

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    // Show local notifications (flutter_local_notifications) while the app is in the foreground.
    UNUserNotificationCenter.current().delegate = self as UNUserNotificationCenterDelegate
    PrivacyScreen.shared.start()
    // At launch, so VoIP pushes reach an app that iOS started in the background for a call.
    VoipCalls.shared.start()
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    if let registrar = engineBridge.pluginRegistry.registrar(forPlugin: "DialogSecurity") {
      let channel = FlutterMethodChannel(name: "dialog/security", binaryMessenger: registrar.messenger())
      channel.setMethodCallHandler(SecurityChannel.handle)
    }
    if let registrar = engineBridge.pluginRegistry.registrar(forPlugin: "DialogVoip") {
      VoipCalls.shared.attach(registrar.messenger())
    }
    if let registrar = engineBridge.pluginRegistry.registrar(forPlugin: "DialogVoiceFx") {
      let channel = FlutterMethodChannel(name: "dialog/voicefx", binaryMessenger: registrar.messenger())
      channel.setMethodCallHandler(VoiceFx.shared.handle)
    }
  }
}

// Live voice effects (crates/voice-fx through mobile/rust/src/voice_fx.rs, C ABI).
@_silgen_name("dialog_fx_configure") private func dialog_fx_configure(_ effect: UInt32, _ denoise: UInt32)
@_silgen_name("dialog_fx_new_call") private func dialog_fx_new_call(_ seed: UInt32)
@_silgen_name("dialog_fx_process") private func dialog_fx_process(_ buf: UnsafeMutablePointer<Float>, _ n: UInt32, _ rate: UInt32)

/// Live voice effects and noise reduction of the call microphone, in flutter_webrtc's capture
/// post-processing (after echo cancellation): the other side only receives the processed
/// voice. With no effect and no noise reduction the samples pass through untouched. Native side
/// of mobile/lib/services/voice_fx.dart.
final class VoiceFx: NSObject, ExternalAudioProcessingDelegate {
  static let shared = VoiceFx()
  private var registered = false
  private var rate: UInt32 = 48000

  /// Hooks into the WebRTC microphone pipeline (once).
  private func register() {
    if registered { return }
    AudioManager.sharedInstance().capturePostProcessingAdapter.addProcessing(self)
    registered = true
  }

  func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let args = call.arguments as? [String: Any]
    switch call.method {
    case "configure":
      register()
      dialog_fx_configure(UInt32(max(0, args?["effect"] as? Int ?? 0)), (args?["denoise"] as? Bool ?? false) ? 1 : 0)
      result(true)
    case "newCall":
      register()
      dialog_fx_new_call(UInt32(truncatingIfNeeded: args?["seed"] as? Int ?? 1))
      result(true)
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  func audioProcessingInitialize(withSampleRate sampleRateHz: Int, channels: Int) {
    rate = UInt32(sampleRateHz)
  }

  func audioProcessingProcess(_ audioBuffer: RTCAudioBuffer) {
    guard audioBuffer.channels > 0, audioBuffer.frames > 0 else { return }
    // Mono microphone: channel 0 holds the full-band samples (16-bit range floats).
    dialog_fx_process(audioBuffer.rawBuffer(forChannel: 0), UInt32(audioBuffer.frames), rate)
  }

  func audioProcessingRelease() {}
}

/// Native side of mobile/lib/services/security.dart.
enum SecurityChannel {
  static func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    switch call.method {
    case "setScreenSecurity":
      PrivacyScreen.shared.enabled = (call.arguments as? Bool) ?? true
      result(nil)
    case "copySensitive":
      let args = call.arguments as? [String: Any]
      let text = args?["text"] as? String ?? ""
      let seconds = args?["seconds"] as? Int ?? 60
      // Local only (no Universal Clipboard to the user's other devices) and removed by the
      // system after `seconds`, without the app having to read the pasteboard back.
      UIPasteboard.general.setItems(
        [["public.utf8-plain-text": text]],
        options: [.localOnly: true, .expirationDate: Date().addingTimeInterval(TimeInterval(seconds))]
      )
      result(nil)
    case "protectDirectory":
      let args = call.arguments as? [String: Any]
      guard let path = args?["path"] as? String else {
        result(nil)
        return
      }
      let level: FileProtectionType
      switch args?["level"] as? String {
      case "complete": level = .complete
      case "completeUnlessOpen": level = .completeUnlessOpen
      default: level = .completeUntilFirstUserAuthentication
      }
      try? FileManager.default.createDirectory(atPath: path, withIntermediateDirectories: true)
      try? FileManager.default.setAttributes([.protectionKey: level], ofItemAtPath: path)
      result(nil)
    default:
      result(FlutterMethodNotImplemented)
    }
  }
}

/// Screen security (setting "Block screenshots", on by default): covers the app while it is
/// inactive (so the app switcher snapshot shows no content) and while the screen is recorded,
/// mirrored or captured. Screenshots show the app as black: iOS has no API to block them, so
/// the window's layer is hosted inside the layer of a secure text field, which the system
/// leaves out of screenshots and recordings (the technique used by banking apps and for
/// view-once media in other messengers). It relies on UIKit behaviour rather than a documented
/// contract: should a future iOS change it, screenshots show the app again, nothing breaks.
final class PrivacyScreen {
  static let shared = PrivacyScreen()

  var enabled = true {
    didSet { update() }
  }

  private var inactive = false
  private var cover: UIView?
  /// The secure field hosting the window's layer (installed once, then toggled).
  private var secureField: UITextField?

  func start() {
    let nc = NotificationCenter.default
    nc.addObserver(forName: UIScene.willDeactivateNotification, object: nil, queue: .main) { [weak self] _ in
      self?.inactive = true
      self?.update()
    }
    nc.addObserver(forName: UIScene.didActivateNotification, object: nil, queue: .main) { [weak self] _ in
      self?.inactive = false
      self?.update()
    }
    nc.addObserver(forName: UIScreen.capturedDidChangeNotification, object: nil, queue: .main) { [weak self] _ in
      self?.update()
    }
  }

  private var window: UIWindow? {
    let windows = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap { $0.windows }
    return windows.first { $0.isKeyWindow } ?? windows.first
  }

  private func update() {
    guard let window = window else { return }
    installSecureLayer(in: window)
    secureField?.isSecureTextEntry = enabled
    let show = enabled && (inactive || window.screen.isCaptured)
    if show {
      if cover == nil {
        // Opaque (a blur would still let shapes and colors through).
        let v = UIView(frame: window.bounds)
        v.backgroundColor = .systemBackground
        v.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        window.addSubview(v)
        cover = v
      }
      if let cover = cover { window.bringSubviewToFront(cover) }
    } else {
      cover?.removeFromSuperview()
      cover = nil
    }
  }

  private func installSecureLayer(in window: UIWindow) {
    guard secureField == nil, let superlayer = window.layer.superlayer else { return }
    let field = UITextField()
    field.isUserInteractionEnabled = false
    field.isSecureTextEntry = true
    // Same frame as the window: the window's layer is drawn relative to the field's canvas.
    field.frame = window.bounds
    field.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    window.addSubview(field)
    window.sendSubviewToBack(field)
    field.layoutIfNeeded()
    superlayer.addSublayer(field.layer)
    // The secure canvas is the field's first sublayer before iOS 17 and its last one since.
    let canvas: CALayer?
    if #available(iOS 17.0, *) {
      canvas = field.layer.sublayers?.last
    } else {
      canvas = field.layer.sublayers?.first
    }
    guard let canvas else {
      field.removeFromSuperview()
      return
    }
    canvas.addSublayer(window.layer)
    secureField = field
  }
}

/// The app's name as shown on the home screen (Info.plist, set by scripts/brand.mjs); used on
/// the system call screen when the caller's name is not shown.
let appDisplayName = Bundle.main.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String
  ?? Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? ""

/// Calls on iOS through CallKit: incoming calls from VoIP pushes (PushKit), and every other
/// call too (outgoing, incoming while the app runs, group calls), reported by Dart. A call that
/// CallKit knows keeps the app running with the screen locked, and CallKit owns the audio
/// session: WebRTC is told when it is activated / deactivated (manual audio), which is what
/// keeps the call's audio going in the background and after interruptions.
/// Dart side: mobile/lib/services/callkit.dart (channel "dialog/voip"); docs/PUSH-SETUP.md.
///
/// A VoIP push carries only a call id and the push tag of the account it is for, never the
/// caller. iOS requires every VoIP push to be reported to CallKit at once (otherwise it stops
/// delivering them), so the call is shown under the app's name (appDisplayName) before Dart
/// runs; Dart then fetches and decrypts the offer and may put the caller's name on the call screen (if notification
/// previews are on). The app's own do-not-disturb (prefs.json "dndUntil", written by Dart) ends
/// the reported call right away: it never rings, and the core records it as missed.
final class VoipCalls: NSObject, PKPushRegistryDelegate, CXProviderDelegate {
  static let shared = VoipCalls()

  private var registry: PKPushRegistry?
  private let provider: CXProvider
  private let controller = CXCallController()
  private var channel: FlutterMethodChannel?
  private var dartReady = false
  /// Events for Dart until it says it is ready (e.g. the app was started by a VoIP push).
  private var pending: [(String, [String: Any])] = []
  private var token: String?
  /// Call id -> CallKit call.
  private var calls: [String: UUID] = [:]
  /// Calls answered on the system screen (or in the app), and our outgoing calls.
  private var answered = Set<String>()
  private var outgoing = Set<String>()
  private var timers: [String: Timer] = [:]
  /// CallKit activated the audio session (and WebRTC audio runs).
  private var audioActive = false
  private var audioTimer: Timer?

  private override init() {
    let config = CXProviderConfiguration()
    config.supportsVideo = true
    config.maximumCallGroups = 1
    config.maximumCallsPerCallGroup = 1
    config.supportedHandleTypes = [.generic]
    // Calls stay out of the phone's Recents list (it would sync to iCloud).
    config.includesCallsInRecents = false
    provider = CXProvider(configuration: config)
    super.init()
    provider.setDelegate(self, queue: nil)
  }

  func start() {
    guard registry == nil else { return }
    let r = PKPushRegistry(queue: .main)
    r.delegate = self
    r.desiredPushTypes = [.voIP]
    registry = r
  }

  func attach(_ messenger: FlutterBinaryMessenger) {
    let ch = FlutterMethodChannel(name: "dialog/voip", binaryMessenger: messenger)
    ch.setMethodCallHandler { [weak self] call, result in
      self?.handle(call, result) ?? result(nil)
    }
    channel = ch
    dartReady = false
  }

  private func send(_ method: String, _ args: [String: Any]) {
    guard dartReady, let ch = channel else {
      pending.append((method, args))
      return
    }
    ch.invokeMethod(method, arguments: args)
  }

  private func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let args = call.arguments as? [String: Any] ?? [:]
    let callId = args["call_id"] as? String ?? ""
    switch call.method {
    case "ready":
      dartReady = true
      let events = pending
      pending = []
      for (method, a) in events { channel?.invokeMethod(method, arguments: a) }
      result(nil)
    case "getVoipToken":
      result(token)
    case "updateCall":
      if let uuid = calls[callId] {
        let update = CXCallUpdate()
        update.localizedCallerName = args["name"] as? String ?? appDisplayName
        update.hasVideo = args["video"] as? Bool ?? false
        provider.reportCall(with: uuid, updated: update)
      }
      result(nil)
    case "answerCall":
      // Answered in the app: the system screen follows (no "answer" event back to Dart).
      cancelTimer(callId)
      if let uuid = calls[callId], !answered.contains(callId) {
        answered.insert(callId)
        controller.request(CXTransaction(action: CXAnswerCallAction(call: uuid))) { _ in }
        ensureAudioSoon()
      }
      result(nil)
    case "callConnected":
      // The media is up: an outgoing call is shown as connected; the audio must be running.
      if let uuid = calls[callId] {
        if outgoing.contains(callId) { provider.reportOutgoingCall(with: uuid, connectedAt: nil) }
        ensureAudioSoon()
      }
      result(nil)
    case "startCall":
      startCall(callId, args, result)
    case "reportIncoming":
      reportIncoming(callId, args, result)
    case "setMuted":
      if let uuid = calls[callId] {
        let muted = args["muted"] as? Bool ?? false
        controller.request(CXTransaction(action: CXSetMutedCallAction(call: uuid, muted: muted))) { _ in }
      }
      result(nil)
    case "endCall":
      if let uuid = calls.removeValue(forKey: callId) {
        cancelTimer(callId)
        answered.remove(callId)
        outgoing.remove(callId)
        if args["local"] as? Bool ?? false {
          // Ended in the app: the system ends it as a user action (falls back to a report).
          controller.request(CXTransaction(action: CXEndCallAction(call: uuid))) { [weak self] error in
            if error != nil {
              DispatchQueue.main.async { self?.provider.reportCall(with: uuid, endedAt: nil, reason: .remoteEnded) }
            }
          }
        } else {
          let reason: CXCallEndedReason = (args["failed"] as? Bool ?? false) ? .failed : .remoteEnded
          provider.reportCall(with: uuid, endedAt: nil, reason: reason)
        }
        releaseAudioIfIdle()
      }
      result(nil)
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  private func callUpdate(name: String, video: Bool) -> CXCallUpdate {
    let update = CXCallUpdate()
    // Generic handle: the caller's identity never reaches the system (Recents are off too).
    update.remoteHandle = CXHandle(type: .generic, value: appDisplayName)
    update.localizedCallerName = name
    update.hasVideo = video
    update.supportsHolding = false
    update.supportsGrouping = false
    update.supportsUngrouping = false
    update.supportsDTMF = false
    return update
  }

  /// An outgoing call (1:1 or group) placed in the app. Result: whether CallKit took it.
  private func startCall(_ callId: String, _ args: [String: Any], _ result: @escaping FlutterResult) {
    guard !callId.isEmpty else { return result(false) }
    if calls[callId] != nil { return result(true) }
    let video = args["video"] as? Bool ?? false
    let name = args["name"] as? String ?? appDisplayName
    let uuid = UUID()
    calls[callId] = uuid
    answered.insert(callId)
    outgoing.insert(callId)
    useCallKitAudio()
    let action = CXStartCallAction(call: uuid, handle: CXHandle(type: .generic, value: appDisplayName))
    action.isVideo = video
    controller.request(CXTransaction(action: action)) { [weak self] error in
      DispatchQueue.main.async {
        guard let self = self else { return }
        if let error = error {
          NSLog("CallKit start call failed: \(error)")
          if self.calls[callId] == uuid {
            self.calls.removeValue(forKey: callId)
            self.answered.remove(callId)
            self.outgoing.remove(callId)
          }
          // Without CallKit, WebRTC drives the audio session itself (as before).
          self.releaseAudioIfIdle(now: true)
          result(false)
          return
        }
        self.provider.reportCall(with: uuid, updated: self.callUpdate(name: name, video: video))
        result(true)
      }
    }
  }

  /// An incoming call (1:1 or group) that reached the running app without a VoIP push. A call
  /// already reported by the push is only updated. Result: whether CallKit shows it.
  private func reportIncoming(_ callId: String, _ args: [String: Any], _ result: @escaping FlutterResult) {
    guard !callId.isEmpty else { return result(false) }
    let video = args["video"] as? Bool ?? false
    let name = args["name"] as? String ?? appDisplayName
    if let uuid = calls[callId] {
      provider.reportCall(with: uuid, updated: callUpdate(name: name, video: video))
      return result(true)
    }
    let uuid = UUID(uuidString: callId) ?? UUID()
    calls[callId] = uuid
    useCallKitAudio()
    provider.reportNewIncomingCall(with: uuid, update: callUpdate(name: name, video: video)) { [weak self] error in
      DispatchQueue.main.async {
        guard let self = self else { return }
        if let error = error {
          // Refused by the system (Focus, blocked): the app rings by itself.
          NSLog("CallKit incoming call refused: \(error)")
          if self.calls[callId] == uuid { self.calls.removeValue(forKey: callId) }
          self.releaseAudioIfIdle(now: true)
          result(false)
          return
        }
        // Dart ends it earlier (ring timeout, answer elsewhere, hang-up).
        if self.calls[callId] == uuid, !self.answered.contains(callId) {
          self.endLater(callId, after: 50, reason: .unanswered)
        }
        result(true)
      }
    }
  }

  // MARK: audio session (CallKit owns it, WebRTC follows)

  /// From now on WebRTC starts its audio only when CallKit has activated the session.
  private func useCallKitAudio() {
    audioTimer?.invalidate()
    audioTimer = nil
    let rtc = RTCAudioSession.sharedInstance()
    if !rtc.useManualAudio {
      rtc.useManualAudio = true
      rtc.isAudioEnabled = audioActive
    }
  }

  /// Safety net: the call is live but CallKit did not activate the audio session (it should
  /// within a moment): WebRTC then starts it itself rather than leaving the call silent.
  private func ensureAudioSoon() {
    audioTimer?.invalidate()
    audioTimer = Timer.scheduledTimer(withTimeInterval: 2.5, repeats: false) { [weak self] _ in
      guard let self = self else { return }
      self.audioTimer = nil
      if !self.audioActive && !self.calls.isEmpty {
        NSLog("CallKit did not activate the audio session: starting WebRTC audio anyway")
        RTCAudioSession.sharedInstance().isAudioEnabled = true
      }
    }
  }

  /// No call left: WebRTC manages its audio by itself again (after CallKit's deactivation).
  private func releaseAudioIfIdle(now: Bool = false) {
    guard calls.isEmpty else { return }
    audioTimer?.invalidate()
    audioTimer = nil
    let reset = { [weak self] in
      guard let self = self, self.calls.isEmpty else { return }
      let rtc = RTCAudioSession.sharedInstance()
      rtc.useManualAudio = false
      rtc.isAudioEnabled = true
    }
    if now {
      reset()
    } else {
      audioTimer = Timer.scheduledTimer(withTimeInterval: 2, repeats: false) { [weak self] _ in
        self?.audioTimer = nil
        reset()
      }
    }
  }

  private func callId(for uuid: UUID) -> String? {
    calls.first { $0.value == uuid }?.key
  }

  private func cancelTimer(_ callId: String) {
    timers.removeValue(forKey: callId)?.invalidate()
  }

  /// Ends the call on the system screen after `seconds` unless Dart took it over.
  private func endLater(_ callId: String, after seconds: TimeInterval, reason: CXCallEndedReason) {
    cancelTimer(callId)
    timers[callId] = Timer.scheduledTimer(withTimeInterval: seconds, repeats: false) { [weak self] _ in
      guard let self = self, let uuid = self.calls.removeValue(forKey: callId) else { return }
      self.timers.removeValue(forKey: callId)
      self.answered.remove(callId)
      self.outgoing.remove(callId)
      self.provider.reportCall(with: uuid, endedAt: nil, reason: reason)
      self.releaseAudioIfIdle()
    }
  }

  /// The app's do-not-disturb, mirrored by Dart in Application Support/prefs.json.
  private func dndActive() -> Bool {
    guard let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first,
          let data = try? Data(contentsOf: dir.appendingPathComponent("prefs.json")),
          let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
          let until = (json["dndUntil"] as? NSNumber)?.doubleValue
    else { return false }
    return until > Date().timeIntervalSince1970 * 1000
  }

  // MARK: PKPushRegistryDelegate

  func pushRegistry(_ registry: PKPushRegistry, didUpdate pushCredentials: PKPushCredentials, for type: PKPushType) {
    let hex = pushCredentials.token.map { String(format: "%02x", $0) }.joined()
    token = hex
    send("voipToken", ["token": hex])
  }

  func pushRegistry(_ registry: PKPushRegistry, didInvalidatePushTokenFor type: PKPushType) {
    token = nil
  }

  func pushRegistry(
    _ registry: PKPushRegistry,
    didReceiveIncomingPushWith payload: PKPushPayload,
    for type: PKPushType,
    completion: @escaping () -> Void
  ) {
    let data = payload.dictionaryPayload
    let callId = (data["call_id"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? UUID().uuidString
    let tag = data["p"] as? String ?? ""
    let known = calls[callId] != nil
    let uuid = calls[callId] ?? UUID(uuidString: callId) ?? UUID()
    calls[callId] = uuid
    if !known { useCallKitAudio() }
    let update = callUpdate(name: appDisplayName, video: false)
    // Reported before anything else (iOS requirement), also when do-not-disturb is on.
    provider.reportNewIncomingCall(with: uuid, update: update) { [weak self] error in
      defer { completion() }
      guard let self = self else { return }
      if error != nil {
        // Already shown (same call pushed twice, or reported by the running app), or refused
        // by the system (Focus, blocked).
        if !known {
          self.calls.removeValue(forKey: callId)
          self.releaseAudioIfIdle(now: true)
        }
        return
      }
      if self.dndActive() {
        self.calls.removeValue(forKey: callId)
        self.provider.reportCall(with: uuid, endedAt: nil, reason: .unanswered)
        self.releaseAudioIfIdle()
        return
      }
      // The caller stops ringing after 45 s; Dart ends it earlier once it has the offer.
      self.endLater(callId, after: 50, reason: .unanswered)
      self.send("incoming", ["call_id": callId, "p": tag])
    }
  }

  // MARK: CXProviderDelegate

  func providerDidReset(_ provider: CXProvider) {
    for t in timers.values { t.invalidate() }
    timers.removeAll()
    for id in calls.keys { send("end", ["call_id": id]) }
    calls.removeAll()
    answered.removeAll()
    outgoing.removeAll()
    releaseAudioIfIdle()
  }

  func provider(_ provider: CXProvider, perform action: CXStartCallAction) {
    // Placed in the app, which is already connecting it.
    provider.reportOutgoingCall(with: action.callUUID, startedConnectingAt: nil)
    action.fulfill()
  }

  func provider(_ provider: CXProvider, perform action: CXAnswerCallAction) {
    if let id = callId(for: action.callUUID), !answered.contains(id) {
      answered.insert(id)
      send("answer", ["call_id": id])
      // Dart connects once the account is open; without it the call cannot go on.
      endLater(id, after: 90, reason: .failed)
    }
    action.fulfill()
  }

  func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
    // Ended on the system screen (or "End & Accept" for another call): the app hangs up.
    // Calls the app ended itself are already gone from `calls`.
    if let id = callId(for: action.callUUID) {
      calls.removeValue(forKey: id)
      cancelTimer(id)
      answered.remove(id)
      outgoing.remove(id)
      send("end", ["call_id": id])
      releaseAudioIfIdle()
    }
    action.fulfill()
  }

  func provider(_ provider: CXProvider, perform action: CXSetMutedCallAction) {
    // Mute button of the system call screen; the app's mute requests come back here too and
    // change nothing there.
    if let id = callId(for: action.callUUID) {
      send("mute", ["call_id": id, "muted": action.isMuted])
    }
    action.fulfill()
  }

  func provider(_ provider: CXProvider, perform action: CXSetHeldCallAction) {
    // Calls cannot be held ("Hold & Accept" for another call): held means muted here; the
    // audio session itself goes to the other call and comes back with didActivate.
    if let id = callId(for: action.callUUID) {
      send("mute", ["call_id": id, "muted": action.isOnHold])
    }
    action.fulfill()
  }

  func provider(_ provider: CXProvider, didActivate audioSession: AVAudioSession) {
    audioActive = true
    let rtc = RTCAudioSession.sharedInstance()
    rtc.audioSessionDidActivate(audioSession)
    rtc.isAudioEnabled = true
  }

  func provider(_ provider: CXProvider, didDeactivate audioSession: AVAudioSession) {
    audioActive = false
    let rtc = RTCAudioSession.sharedInstance()
    rtc.audioSessionDidDeactivate(audioSession)
    rtc.isAudioEnabled = false
  }
}
