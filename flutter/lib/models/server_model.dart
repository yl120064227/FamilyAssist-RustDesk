import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/main.dart';
import 'package:flutter_hbb/mobile/pages/settings_page.dart';
import 'package:flutter_hbb/models/chat_model.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:get/get.dart';
import 'package:window_manager/window_manager.dart';

import '../common.dart';
import '../common/formatter/id_formatter.dart';
import '../desktop/pages/server_page.dart' as desktop;
import '../desktop/widgets/tabbar_widget.dart';
import '../mobile/pages/server_page.dart';
import 'model.dart';

const kLoginDialogTag = "LOGIN";

const kUseTemporaryPassword = "use-temporary-password";
const kUsePermanentPassword = "use-permanent-password";
const kUseBothPasswords = "use-both-passwords";

class ServerModel with ChangeNotifier {
  bool _isStart = false; // Android MainService status
  bool _mediaOk = false;
  bool _inputOk = false;
  bool _audioOk = false;
  bool _fileOk = false;
  bool _clipboardOk = false;
  bool _showElevation = false;
  bool hideCm = false;
  int _connectStatus = 0; // Rendezvous Server status
  String _verificationMethod = "";
  String _temporaryPasswordLength = "";
  bool _allowNumericOneTimePassword = false;
  String _approveMode = "";
  int _zeroClientLengthCounter = 0;

  late String _emptyIdShow;
  late final IDTextEditingController _serverId;
  final _serverPasswd =
      TextEditingController(text: translate("Generating ..."));

  final tabController = DesktopTabController(tabType: DesktopTabType.cm);

  final List<Client> _clients = [];

  Timer? cmHiddenTimer;
  Timer? _serverTimer;
  bool _timerCallbackRunning = false;
  bool _disposed = false;
  // Incremented whenever a service transition starts. An older async start
  // must not finish after a newer stop (or restart) request.
  int _serviceOperation = 0;
  int _keepScreenOnGeneration = 0;

  // 家庭守卫（WP5）：用户显式关闭输入控制后，抑制无障碍绑定状态事件的自动回弹。
  // 系统级 enabled_accessibility_services 仍注册时，InputService 会被系统重新
  // 绑定并触发 on_state_changed(input=true)——若不抑制，开关在数秒内被拉回 ON
  //（G1 动态测试 T2 实测）。用户主动开启路径（showInputWarnAlert 确认后返回）
  // 会清除此抑制，正常开启流程不受影响。
  DateTime? _suppressAutoInputOnUntil;
  bool? _familyGuardCache;

  final _wakelockKey = UniqueKey();

  bool get isStart => _isStart;

  bool get mediaOk => _mediaOk;

  bool get inputOk => _inputOk;

  bool get audioOk => _audioOk;

  bool get fileOk => _fileOk;

  bool get clipboardOk => _clipboardOk;

  bool get showElevation => _showElevation;

  int get connectStatus => _connectStatus;

  String get verificationMethod {
    final index = [
      kUseTemporaryPassword,
      kUsePermanentPassword,
      kUseBothPasswords
    ].indexOf(_verificationMethod);
    if (index < 0) {
      return kUseBothPasswords;
    }
    return _verificationMethod;
  }

  String get approveMode => _approveMode;

  setVerificationMethod(String method) async {
    await bind.mainSetOption(key: kOptionVerificationMethod, value: method);
    /*
    if (method != kUsePermanentPassword) {
      await bind.mainSetOption(
          key: 'allow-hide-cm', value: bool2option('allow-hide-cm', false));
    }
    */
  }

  String get temporaryPasswordLength {
    final lengthIndex = ["6", "8", "10"].indexOf(_temporaryPasswordLength);
    if (lengthIndex < 0) {
      return "6";
    }
    return _temporaryPasswordLength;
  }

  setTemporaryPasswordLength(String length) async {
    await bind.mainSetOption(key: "temporary-password-length", value: length);
  }

  setApproveMode(String mode) async {
    await bind.mainSetOption(key: kOptionApproveMode, value: mode);
    /*
    if (mode != 'password') {
      await bind.mainSetOption(
          key: 'allow-hide-cm', value: bool2option('allow-hide-cm', false));
    }
    */
  }

  bool get allowNumericOneTimePassword => _allowNumericOneTimePassword;
  switchAllowNumericOneTimePassword() async {
    await mainSetBoolOption(
        kOptionAllowNumericOneTimePassword, !_allowNumericOneTimePassword);
  }

  TextEditingController get serverId => _serverId;

  TextEditingController get serverPasswd => _serverPasswd;

  List<Client> get clients => _clients;

  final controller = ScrollController();

  WeakReference<FFI> parent;

  ServerModel(this.parent) {
    _emptyIdShow = translate("Generating ...");
    _serverId = IDTextEditingController(text: _emptyIdShow);

    /*
    // initital _hideCm at startup
    final verificationMethod =
        bind.mainGetOptionSync(key: kOptionVerificationMethod);
    final approveMode = bind.mainGetOptionSync(key: kOptionApproveMode);
    _hideCm = option2bool(
        'allow-hide-cm', bind.mainGetOptionSync(key: 'allow-hide-cm'));
    if (!(approveMode == 'password' &&
        verificationMethod == kUsePermanentPassword)) {
      _hideCm = false;
    }
    */

    Future<void> timerCallback() async {
      if (_disposed || _timerCallbackRunning) return;
      _timerCallbackRunning = true;
      try {
        final connectionStatus = jsonDecode(
                await bind.mainGetConnectStatus()) as Map<String, dynamic>;
        if (_disposed) return;
        final statusNum = connectionStatus['status_num'] as int;
        if (statusNum != _connectStatus) {
          _connectStatus = statusNum;
          notifyListeners();
        }

        if (desktopType == DesktopType.cm) {
          final res = await bind.cmCheckClientsLength(length: _clients.length);
          if (_disposed) return;
          if (res != null) {
            debugPrint("clients not match!");
            await updateClientState(res);
          } else {
            if (_clients.isEmpty) {
              hideCmWindow();
              if (_zeroClientLengthCounter++ == 12) {
                // 6 seconds
                await windowManager.close();
              }
            } else {
              _zeroClientLengthCounter = 0;
              if (!hideCm) showCmWindow();
            }
          }
        }

        if (!_disposed) {
          await updatePasswordModel();
        }
      } catch (error, stackTrace) {
        if (!_disposed) {
          debugPrint("Server status poll failed: $error\n$stackTrace");
        }
      } finally {
        _timerCallbackRunning = false;
      }
    }
    if (!isTest) {
      Future.delayed(Duration.zero, () async {
        if (await bind.optionSynced()) {
          await timerCallback();
        }
      });
      _serverTimer = Timer.periodic(Duration(milliseconds: 500), (_) {
        timerCallback();
      });
    }

    // Initial keyboard status is off on mobile
    if (isMobile) {
      bind.mainSetOption(key: kOptionEnableKeyboard, value: 'N');
    }
  }

  @override
  void dispose() {
    _disposed = true;
    _serverTimer?.cancel();
    _serverTimer = null;
    cmHiddenTimer?.cancel();
    cmHiddenTimer = null;
    WakelockManager.disable(_wakelockKey);
    controller.dispose();
    _serverPasswd.dispose();
    _serverId.dispose();
    super.dispose();
  }

  /// 1. check android permission
  /// 2. check config
  /// audio true by default (if permission on) (false default < Android 10)
  /// file true by default (if permission on)
  Future<void> checkAndroidPermission() async {
    if (_disposed) return;
    // audio
    if (androidVersion < 30 ||
        !await AndroidPermissionManager.check(kRecordAudio)) {
      _audioOk = false;
      await bind.mainSetOption(key: kOptionEnableAudio, value: "N");
    } else {
      final audioOption = await bind.mainGetOption(key: kOptionEnableAudio);
      if (_disposed) return;
      _audioOk = audioOption != 'N';
    }
    if (_disposed) return;

    // file
    if (!await AndroidPermissionManager.check(kManageExternalStorage)) {
      _fileOk = false;
      await bind.mainSetOption(key: kOptionEnableFileTransfer, value: "N");
    } else {
      final fileOption =
          await bind.mainGetOption(key: kOptionEnableFileTransfer);
      if (_disposed) return;
      _fileOk = fileOption != 'N';
    }
    if (_disposed) return;

    // clipboard
    final clipOption = await bind.mainGetOption(key: kOptionEnableClipboard);
    if (_disposed) return;
    _clipboardOk = clipOption != 'N';

    if (!_disposed) {
      notifyListeners();
    }
  }

  updatePasswordModel() async {
    if (_disposed) return;
    var update = false;
    final temporaryPassword = await bind.mainGetTemporaryPassword();
    if (_disposed) return;
    final verificationMethod =
        await bind.mainGetOption(key: kOptionVerificationMethod);
    if (_disposed) return;
    final temporaryPasswordLength =
        await bind.mainGetOption(key: "temporary-password-length");
    if (_disposed) return;
    final approveMode = await bind.mainGetOption(key: kOptionApproveMode);
    if (_disposed) return;
    final numericOneTimePassword =
        await mainGetBoolOption(kOptionAllowNumericOneTimePassword);
    if (_disposed) return;
    /*
    var hideCm = option2bool(
        'allow-hide-cm', await bind.mainGetOption(key: 'allow-hide-cm'));
    if (!(approveMode == 'password' &&
        verificationMethod == kUsePermanentPassword)) {
      hideCm = false;
    }
    */
    if (_approveMode != approveMode) {
      _approveMode = approveMode;
      update = true;
    }
    var stopped = await mainGetBoolOption(kOptionStopService);
    final oldPwdText = _serverPasswd.text;
    if (stopped ||
        verificationMethod == kUsePermanentPassword ||
        _approveMode == 'click') {
      _serverPasswd.text = '-';
    } else {
      if (_serverPasswd.text != temporaryPassword &&
          temporaryPassword.isNotEmpty) {
        _serverPasswd.text = temporaryPassword;
      }
    }
    if (oldPwdText != _serverPasswd.text) {
      update = true;
    }
    if (_verificationMethod != verificationMethod) {
      _verificationMethod = verificationMethod;
      update = true;
    }
    if (_temporaryPasswordLength != temporaryPasswordLength) {
      if (_temporaryPasswordLength.isNotEmpty) {
        bind.mainUpdateTemporaryPassword();
      }
      _temporaryPasswordLength = temporaryPasswordLength;
      update = true;
    }
    if (_allowNumericOneTimePassword != numericOneTimePassword) {
      _allowNumericOneTimePassword = numericOneTimePassword;
      update = true;
    }
    /*
    if (_hideCm != hideCm) {
      _hideCm = hideCm;
      if (desktopType == DesktopType.cm) {
        if (hideCm) {
          await hideCmWindow();
        } else {
          await showCmWindow();
        }
      }
      update = true;
    }
    */
    if (update && !_disposed) {
      notifyListeners();
    }
  }

  Future<void> toggleAudio() async {
    if (clients.any((c) => !c.disconnected)) {
      await showClientsMayNotBeChangedAlert(parent.target);
      return;
    }
    if (!_audioOk && !await AndroidPermissionManager.check(kRecordAudio)) {
      final res = await AndroidPermissionManager.request(kRecordAudio);
      if (!res) {
        showToast(translate('Failed'));
        return;
      }
    }

    _audioOk = !_audioOk;
    await bind.mainSetOption(
        key: kOptionEnableAudio, value: _audioOk ? defaultOptionYes : 'N');
    if (!_disposed) notifyListeners();
  }

  Future<void> toggleFile() async {
    if (clients.any((c) => !c.disconnected)) {
      await showClientsMayNotBeChangedAlert(parent.target);
      return;
    }
    if (!_fileOk &&
        !await AndroidPermissionManager.check(kManageExternalStorage)) {
      final res =
          await AndroidPermissionManager.request(kManageExternalStorage);
      if (!res) {
        showToast(translate('Failed'));
        return;
      }
    }

    _fileOk = !_fileOk;
    await bind.mainSetOption(
        key: kOptionEnableFileTransfer,
        value: _fileOk ? defaultOptionYes : 'N');
    if (!_disposed) notifyListeners();
  }

  Future<void> toggleClipboard() async {
    if (clients.any((c) => !c.disconnected)) {
      await showClientsMayNotBeChangedAlert(parent.target);
      return;
    }
    _clipboardOk = !_clipboardOk;
    await bind.mainSetOption(
        key: kOptionEnableClipboard,
        value: _clipboardOk ? defaultOptionYes : 'N');
    if (!_disposed) notifyListeners();
  }

  Future<void> toggleInput() async {
    if (clients.any((c) => !c.disconnected)) {
      await showClientsMayNotBeChangedAlert(parent.target);
      return;
    }
    if (_inputOk) {
      parent.target?.invokeMethod("stop_input");
      await bind.mainSetOption(key: kOptionEnableKeyboard, value: 'N');
      // 家庭守卫（WP5）：记录用户显式关闭时刻——后续 60s 内的 input=true 绑定
      // 事件视为系统残留回弹并抑制
      _suppressAutoInputOnUntil = DateTime.now().add(
        const Duration(seconds: 60),
      );
    } else {
      if (parent.target != null) {
        /// the result of toggle-on depends on user actions in the settings page.
        /// handle result, see [ServerModel.changeStatue]
        // 家庭守卫（WP5）：用户主动开启——清除抑制，允许绑定事件生效
        _suppressAutoInputOnUntil = null;
        showInputWarnAlert(parent.target!);
      }
    }
  }

  Future<bool> checkRequestNotificationPermission() async {
    debugPrint("androidVersion $androidVersion");
    if (androidVersion < 33) {
      return true;
    }
    if (await AndroidPermissionManager.check(kAndroid13Notification)) {
      debugPrint("notification permission already granted");
      return true;
    }
    var res = await AndroidPermissionManager.request(kAndroid13Notification);
    debugPrint("notification permission request result: $res");
    return res;
  }

  Future<bool> checkFloatingWindowPermission() async {
    debugPrint("androidVersion $androidVersion");
    if (androidVersion < 23) {
      return false;
    }
    if (await AndroidPermissionManager.check(kSystemAlertWindow)) {
      debugPrint("alert window permission already granted");
      return true;
    }
    var res = await AndroidPermissionManager.request(kSystemAlertWindow);
    debugPrint("alert window permission request result: $res");
    return res;
  }

  /// Toggle the screen sharing service.
  toggleService() async {
    if (_isStart) {
      final res = await parent.target?.dialogManager
          .show<bool>((setState, close, context) {
        submit() => close(true);
        return CustomAlertDialog(
          title: Row(children: [
            const Icon(Icons.warning_amber_sharp,
                color: Colors.redAccent, size: 28),
            const SizedBox(width: 10),
            Text(translate("Warning")),
          ]),
          content: Text(translate("android_stop_service_tip")),
          actions: [
            TextButton(onPressed: close, child: Text(translate("Cancel"))),
            TextButton(onPressed: submit, child: Text(translate("OK"))),
          ],
          onSubmit: submit,
          onCancel: close,
        );
      });
      if (res == true) {
        await stopService();
      }
    } else {
      await checkRequestNotificationPermission();
      if (bind.mainGetLocalOption(key: kOptionDisableFloatingWindow) != 'Y') {
        await checkFloatingWindowPermission();
      }
      if (!await AndroidPermissionManager.check(kManageExternalStorage)) {
        await AndroidPermissionManager.request(kManageExternalStorage);
      }
      final res = await parent.target?.dialogManager
          .show<bool>((setState, close, context) {
        submit() => close(true);
        return CustomAlertDialog(
          title: Row(children: [
            const Icon(Icons.warning_amber_sharp,
                color: Colors.redAccent, size: 28),
            const SizedBox(width: 10),
            Text(translate("Warning")),
          ]),
          content: Text(translate("android_service_will_start_tip")),
          actions: [
            dialogButton("Cancel", onPressed: close, isOutline: true),
            dialogButton("OK", onPressed: submit),
          ],
          onSubmit: submit,
          onCancel: close,
        );
      });
      if (res == true) {
        await startService();
      }
    }
  }

  /// Start the screen sharing service.
  Future<void> startService() async {
    if (_disposed || _isStart) return;

    final operation = ++_serviceOperation;
    _isStart = true;
    notifyListeners();
    try {
      parent.target?.ffiModel.updateEventListener(parent.target!.sessionId, "");
      await parent.target?.invokeMethod("init_service");
      if (_disposed || operation != _serviceOperation || !_isStart) {
        await _rollbackServiceStart(operation);
        return;
      }
      // ugly is here, because for desktop, below is useless
      await bind.mainStartService();
      if (_disposed || operation != _serviceOperation || !_isStart) {
        await _rollbackServiceStart(operation);
        return;
      }
      await updateClientState();
      if (_disposed || operation != _serviceOperation || !_isStart) {
        await _rollbackServiceStart(operation);
        return;
      }
      if (isAndroid) {
        androidUpdatekeepScreenOn();
      }
    } catch (error, stackTrace) {
      debugPrint("Failed to start service: $error\n$stackTrace");
      final superseded = operation != _serviceOperation || !_isStart;
      if (!superseded) {
        _isStart = false;
      }
      await _rollbackServiceStart(operation);
      if (superseded) return;
      if (!_disposed) {
        notifyListeners();
        showToast(translate('Failed'));
      }
    }
  }

  Future<void> _rollbackServiceStart(int operation) async {
    // A newer start owns the service now; an older failed operation must not
    // tear it down while cleaning up its own state.
    if (operation != _serviceOperation && _isStart) return;
    try {
      await parent.target?.invokeMethod("stop_service");
      await bind.mainStopService();
    } catch (error, stackTrace) {
      debugPrint("Failed to roll back service start: $error\n$stackTrace");
    }
  }

  /// Stop the screen sharing service.
  Future<void> stopService() async {
    if (_disposed) return;
    ++_serviceOperation;
    _isStart = false;
    try {
      await closeAll();
      await parent.target?.invokeMethod("stop_service");
      await bind.mainStopService();
    } catch (error, stackTrace) {
      debugPrint("Failed to stop service: $error\n$stackTrace");
    } finally {
      if (!_disposed) {
        notifyListeners();
        // for androidUpdatekeepScreenOn only
        WakelockManager.disable(_wakelockKey);
      }
    }
  }

  fetchID() async {
    final id = await bind.mainGetMyId();
    if (id != _serverId.id) {
      _serverId.id = id;
      notifyListeners();
    }
  }

  Future<void> changeStatue(String name, bool value) async {
    if (_disposed) return;
    debugPrint("changeStatue value $value");
    try {
      switch (name) {
        case "media":
          _mediaOk = value;
          if (value) {
            if (!_isStart) {
              await startService();
            }
          } else {
            // MediaProjection can be revoked outside Flutter. Clear the local
            // service state and close stale sessions so a later retry can start.
            _isStart = false;
            if (_clients.isNotEmpty) {
              await closeAll();
            }
            androidUpdatekeepScreenOn();
          }
          break;
        case "input":
          // 家庭守卫（WP5）：抑制系统级无障碍残留绑定导致的自动回弹——
          // 用户刚显式关闭输入控制（60s 窗口内）时，input=true 事件被忽略，
          // 选项与 UI 状态保持关闭（键盘权限 false）。关闭方向（false）不受影响。
          if (value && _suppressAutoInputOnUntil != null) {
            if (DateTime.now().isBefore(_suppressAutoInputOnUntil!)) {
              if (_familyGuardEnabled()) {
                debugPrint(
                    "family guard: suppressed auto input-on from a11y rebind");
                return;
              }
            }
            _suppressAutoInputOnUntil = null;
          }
          if (_inputOk != value) {
            await bind.mainSetOption(
                key: kOptionEnableKeyboard,
                value: value ? defaultOptionYes : 'N');
          }
          _inputOk = value;
          break;
        default:
          return;
      }
    } catch (error, stackTrace) {
      debugPrint("Failed to update Android service state: $error\n$stackTrace");
    } finally {
      if (!_disposed) notifyListeners();
    }
  }

  bool _familyGuardEnabled() {
    // 缓存一次：本地选项不参与同步，进程内不会变
    _familyGuardCache ??= bind.mainGetLocalOption(key: 'family-guard') != 'N';
    return _familyGuardCache!;
  }

  // force
  updateClientState([String? json]) async {
    if (isTest || _disposed) return;
    final res = json ?? await bind.cmGetClientsState();
    if (_disposed) return;
    List<dynamic> clientsJson;
    try {
      clientsJson = jsonDecode(res);
    } catch (e) {
      debugPrint("Failed to decode clientsJson: '$res', error $e");
      return;
    }

    final oldClientLenght = _clients.length;
    _clients.clear();
    tabController.state.value.tabs.clear();

    for (var clientJson in clientsJson) {
      try {
        final client = Client.fromJson(clientJson);
        _clients.add(client);
        _addTab(client);
      } catch (e) {
        debugPrint("Failed to decode clientJson '$clientJson', error $e");
      }
    }
    if (desktopType == DesktopType.cm) {
      if (_clients.isEmpty) {
        hideCmWindow();
      } else if (!hideCm) {
        showCmWindow();
      }
    }
    if (!_disposed) {
      // The list length can stay the same while a connection changes state.
      // Refresh listeners after rebuilding the complete client snapshot.
      notifyListeners();
      if (_clients.length != oldClientLenght && isAndroid) {
        androidUpdatekeepScreenOn();
      }
    }
  }

  void addConnection(Map<String, dynamic> evt) {
    try {
      final client = Client.fromJson(jsonDecode(evt["client"]));
      if (client.authorized) {
        parent.target?.dialogManager.dismissByTag(getLoginDialogTag(client.id));
        final index = _clients.indexWhere((c) => c.id == client.id);
        if (index < 0) {
          _clients.add(client);
        } else {
          if (_clients[index].authorized) {
            _clients[index].privacyMode = client.privacyMode;
            notifyListeners();
            return;
          }
          _clients[index].authorized = true;
          _clients[index].privacyMode = client.privacyMode;
        }
      } else {
        final index = _clients.indexWhere((c) => c.id == client.id);
        if (index >= 0) {
          _clients[index].privacyMode = client.privacyMode;
          notifyListeners();
          return;
        }
        _clients.add(client);
      }
      _addTab(client);
      // remove disconnected
      final index_disconnected = _clients
          .indexWhere((c) => c.disconnected && c.peerId == client.peerId);
      if (index_disconnected >= 0) {
        _clients.removeAt(index_disconnected);
        tabController.remove(index_disconnected);
      }
      if (desktopType == DesktopType.cm && !hideCm) {
        showCmWindow();
      }
      scrollToBottom();
      notifyListeners();
      if (isAndroid && !client.authorized) {
        // FamilyAssist: the consent dialog lives inside this activity. If the
        // app was backgrounded (elder pressed HOME), it would never be seen
        // and the request would silently auto-reject after the timeout.
        parent.target?.invokeMethod(AndroidChannel.kBringToFront);
        showLoginDialog(client);
      }
      if (isAndroid) androidUpdatekeepScreenOn();
    } catch (e) {
      debugPrint("Failed to call loginRequest,error:$e");
    }
  }

  void _addTab(Client client) {
    tabController.add(TabInfo(
        key: client.id.toString(),
        label: client.name,
        closable: false,
        onTap: () {},
        page: desktop.buildConnectionCard(client)));
    Future.delayed(Duration.zero, () async {
      if (!_disposed && !hideCm) {
        await windowOnTop(null);
      }
    });
    // Only do the hidden task when on Desktop.
    if (client.authorized && isDesktop) {
      cmHiddenTimer = Timer(const Duration(seconds: 3), () {
        if (!_disposed && !hideCm) {
          windowManager.minimize();
        }
        cmHiddenTimer = null;
      });
    }
    parent.target?.chatModel
        .updateConnIdOfKey(MessageKey(client.peerId, client.id));
  }

  void showLoginDialog(Client client) {
    showClientDialog(
      client,
      client.isFileTransfer
          ? "Transfer file"
          : client.isViewCamera
              ? "View camera"
              : client.isTerminal
                  ? "Terminal"
                  : "Share screen",
      'Do you accept?',
      'android_new_connection_tip',
      () => sendLoginResponse(client, false),
      () => sendLoginResponse(client, true),
    );
  }

  handleVoiceCall(Client client, bool accept) {
    if (_disposed) return;
    parent.target?.invokeMethod("cancel_notification", client.id);
    bind.cmHandleIncomingVoiceCall(id: client.id, accept: accept);
  }

  showVoiceCallDialog(Client client) {
    showClientDialog(
      client,
      'Voice call',
      'Do you accept?',
      'android_new_voice_call_tip',
      () => handleVoiceCall(client, false),
      () => handleVoiceCall(client, true),
    );
  }

  showClientDialog(Client client, String title, String contentTitle,
      String content, VoidCallback onCancel, VoidCallback onSubmit) {
    if (_disposed) return;
    parent.target?.dialogManager.show((setState, close, context) {
      cancel() {
        onCancel();
        close();
      }

      submit() {
        onSubmit();
        close();
      }

      return CustomAlertDialog(
        title:
            Row(mainAxisAlignment: MainAxisAlignment.spaceBetween, children: [
          Text(translate(title)),
          IconButton(onPressed: close, icon: const Icon(Icons.close))
        ]),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          mainAxisAlignment: MainAxisAlignment.center,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(translate(contentTitle)),
            ClientInfo(client),
            Text(
              translate(content),
              style: Theme.of(globalKey.currentContext!).textTheme.bodyMedium,
            ),
          ],
        ),
        actions: [
          dialogButton("Dismiss", onPressed: cancel, isOutline: true),
          if (approveMode != 'password')
            dialogButton("Accept", onPressed: submit),
        ],
        onSubmit: submit,
        onCancel: cancel,
      );
    }, tag: getLoginDialogTag(client.id));
  }

  void scrollToBottom() {
    if (isDesktop || _disposed) return;
    Future.delayed(const Duration(milliseconds: 200), () {
      if (_disposed || !controller.hasClients) return;
      controller.animateTo(
        controller.position.maxScrollExtent,
        duration: const Duration(milliseconds: 200),
        curve: Curves.fastLinearToSlowEaseIn,
      );
    });
  }

  Future<void> sendLoginResponse(Client client, bool res) async {
    if (_disposed) return;
    try {
      await bind.cmLoginRes(connId: client.id, res: res);
    } catch (error, stackTrace) {
      debugPrint("Failed to answer login request: $error\n$stackTrace");
      return;
    }
    parent.target?.invokeMethod("cancel_notification", client.id);
    if (res) {
      if (!client.isFileTransfer && !client.isTerminal) {
        parent.target?.invokeMethod("start_capture");
      }
      client.authorized = true;
      if (!_disposed) notifyListeners();
    } else {
      final index = _clients.indexWhere((item) => item.id == client.id);
      if (index >= 0) {
        tabController.remove(index);
        _clients.removeAt(index);
      }
      if (isAndroid) androidUpdatekeepScreenOn();
    }
  }

  void onClientRemove(Map<String, dynamic> evt) {
    try {
      final id = int.tryParse(evt['id']?.toString() ?? '');
      if (id == null) return;
      final closeValue = evt['close'];
      final close =
          closeValue == true || closeValue?.toString().toLowerCase() == 'true';
      if (_clients.any((c) => c.id == id)) {
        final index = _clients.indexWhere((client) => client.id == id);
        if (index >= 0) {
          if (close) {
            _clients.removeAt(index);
            tabController.remove(index);
          } else {
            _clients[index].disconnected = true;
          }
        }
        parent.target?.dialogManager.dismissByTag(getLoginDialogTag(id));
        parent.target?.invokeMethod("cancel_notification", id);
      }
      if (desktopType == DesktopType.cm && _clients.isEmpty) {
        hideCmWindow();
      }
      if (isAndroid) androidUpdatekeepScreenOn();
      if (!_disposed) notifyListeners();
    } catch (e) {
      debugPrint("onClientRemove failed,error:$e");
    }
  }

  Future<void> closeAll() async {
    if (_disposed) return;
    final clients = List<Client>.of(_clients);
    try {
      await Future.wait(
          clients.map((client) => bind.cmCloseConnection(connId: client.id)),
          eagerError: false);
    } catch (error, stackTrace) {
      debugPrint("Failed to close all connections: $error\n$stackTrace");
    } finally {
      if (!_disposed) {
        _clients.clear();
        tabController.state.value.tabs.clear();
        if (isAndroid) androidUpdatekeepScreenOn();
      }
    }
  }

  void jumpTo(int id) {
    if (_disposed) return;
    final index = _clients.indexWhere((client) => client.id == id);
    if (index >= 0) {
      tabController.jumpTo(index);
    }
  }

  void setShowElevation(bool show) {
    if (_disposed || _showElevation == show) return;
    _showElevation = show;
    notifyListeners();
  }

  void updateVoiceCallState(Map<String, dynamic> evt) {
    try {
      final client = Client.fromJson(jsonDecode(evt["client"]));
      final index = _clients.indexWhere((element) => element.id == client.id);
      if (index != -1) {
        _clients[index].inVoiceCall = client.inVoiceCall;
        _clients[index].incomingVoiceCall = client.incomingVoiceCall;
        if (client.incomingVoiceCall) {
          if (isAndroid) {
            showVoiceCallDialog(client);
          } else {
            // Has incoming phone call, let's set the window on top.
            Future.delayed(Duration.zero, () {
              if (!_disposed) windowOnTop(null);
            });
          }
        }
        if (!_disposed) notifyListeners();
      }
    } catch (e) {
      debugPrint("updateVoiceCallState failed: $e");
    }
  }

  void androidUpdatekeepScreenOn() async {
    if (!isAndroid || _disposed) return;
    final generation = ++_keepScreenOnGeneration;
    var floatingWindowDisabled =
        bind.mainGetLocalOption(key: kOptionDisableFloatingWindow) == "Y" ||
            !await AndroidPermissionManager.check(kSystemAlertWindow);
    if (_disposed || generation != _keepScreenOnGeneration) return;
    final keepScreenOn = floatingWindowDisabled
        ? KeepScreenOn.never
        : optionToKeepScreenOn(
            bind.mainGetLocalOption(key: kOptionKeepScreenOn));
    final on = ((keepScreenOn == KeepScreenOn.serviceOn) && _isStart) ||
        (keepScreenOn == KeepScreenOn.duringControlled &&
            _clients.any((e) => !e.disconnected));
    if (on) {
      WakelockManager.enable(_wakelockKey, isServer: true);
    } else {
      WakelockManager.disable(_wakelockKey);
    }
  }
}

enum ClientType {
  remote,
  file,
  camera,
  portForward,
  terminal,
}

class Client {
  static int _intValue(dynamic value) {
    if (value is int) return value;
    if (value is num) return value.toInt();
    return int.tryParse(value?.toString() ?? '') ?? 0;
  }

  static bool _boolValue(dynamic value) => value == true;

  static String _stringValue(dynamic value) => value?.toString() ?? '';

  int id = 0; // client connections inner count id
  bool authorized = false;
  bool isFileTransfer = false;
  bool isViewCamera = false;
  bool isTerminal = false;
  String portForward = "";
  String name = "";
  String avatar = "";
  String peerId = ""; // peer user's id,show at app
  bool keyboard = false;
  bool clipboard = false;
  bool audio = false;
  bool file = false;
  bool restart = false;
  bool recording = false;
  bool blockInput = false;
  bool privacyMode = false;
  bool disconnected = false;
  bool fromSwitch = false;
  bool inVoiceCall = false;
  bool incomingVoiceCall = false;

  RxInt unreadChatMessageCount = 0.obs;

  Client(this.id, this.authorized, this.isFileTransfer, this.isViewCamera,
      this.name, this.peerId, this.keyboard, this.clipboard, this.audio);

  Client.fromJson(Map<String, dynamic> json) {
    // Connection events are produced by different native versions. Treat
    // missing or differently typed optional fields as defaults instead of
    // dropping the whole event because of a cast exception.
    id = _intValue(json['id']);
    authorized = _boolValue(json['authorized']);
    isFileTransfer = _boolValue(json['is_file_transfer']);
    isViewCamera = _boolValue(json['is_view_camera']);
    isTerminal = _boolValue(json['is_terminal']);
    portForward = _stringValue(json['port_forward']);
    name = _stringValue(json['name']);
    avatar = _stringValue(json['avatar']);
    peerId = _stringValue(json['peer_id']);
    keyboard = _boolValue(json['keyboard']);
    clipboard = _boolValue(json['clipboard']);
    audio = _boolValue(json['audio']);
    file = _boolValue(json['file']);
    restart = _boolValue(json['restart']);
    recording = _boolValue(json['recording']);
    blockInput = _boolValue(json['block_input']);
    privacyMode = _boolValue(json['privacy_mode']);
    disconnected = _boolValue(json['disconnected']);
    fromSwitch = _boolValue(json['from_switch']);
    inVoiceCall = _boolValue(json['in_voice_call']);
    incomingVoiceCall = _boolValue(json['incoming_voice_call']);
  }

  Map<String, dynamic> toJson() {
    final Map<String, dynamic> data = <String, dynamic>{};
    data['id'] = id;
    data['authorized'] = authorized;
    data['is_file_transfer'] = isFileTransfer;
    data['is_view_camera'] = isViewCamera;
    data['is_terminal'] = isTerminal;
    data['port_forward'] = portForward;
    data['name'] = name;
    data['avatar'] = avatar;
    data['peer_id'] = peerId;
    data['keyboard'] = keyboard;
    data['clipboard'] = clipboard;
    data['audio'] = audio;
    data['file'] = file;
    data['restart'] = restart;
    data['recording'] = recording;
    data['block_input'] = blockInput;
    data['privacy_mode'] = privacyMode;
    data['disconnected'] = disconnected;
    data['from_switch'] = fromSwitch;
    data['in_voice_call'] = inVoiceCall;
    data['incoming_voice_call'] = incomingVoiceCall;
    return data;
  }

  ClientType type_() {
    if (isFileTransfer) {
      return ClientType.file;
    } else if (isViewCamera) {
      return ClientType.camera;
    } else if (isTerminal) {
      return ClientType.terminal;
    } else if (portForward.isNotEmpty) {
      return ClientType.portForward;
    } else {
      return ClientType.remote;
    }
  }
}

String getLoginDialogTag(int id) {
  return kLoginDialogTag + id.toString();
}

showInputWarnAlert(FFI ffi) {
  ffi.dialogManager.show((setState, close, context) {
    submit() {
      AndroidPermissionManager.startAction(kActionAccessibilitySettings);
      close();
    }

    return CustomAlertDialog(
      title: Text(translate("How to get Android input permission?")),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(translate("android_input_permission_tip1")),
          const SizedBox(height: 10),
          Text(translate("android_input_permission_tip2")),
        ],
      ),
      actions: [
        dialogButton("Cancel", onPressed: close, isOutline: true),
        dialogButton("Open System Setting", onPressed: submit),
      ],
      onSubmit: submit,
      onCancel: close,
    );
  });
}

Future<void> showClientsMayNotBeChangedAlert(FFI? ffi) async {
  await ffi?.dialogManager.show((setState, close, context) {
    return CustomAlertDialog(
      title: Text(translate("Permissions")),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(translate("android_permission_may_not_change_tip")),
        ],
      ),
      actions: [
        dialogButton("OK", onPressed: close),
      ],
      onSubmit: close,
      onCancel: close,
    );
  });
}
