// The local API over the Rust bridge (docs/phase-1-api.md): each call goes
// through `Client.call` as JSON text, and events come from `Client.eventsJson`.
// The generated [RpcPostApi] reads and writes the JSON on either side.

import 'dart:convert';

import 'package:post/src/rust/api/client.dart';

import 'post_api.dart';

/// A [PostTransport] over the bridge's [Client].
final class BridgeTransport implements PostTransport {
  BridgeTransport(this.client);

  /// The open profile.
  final Client client;

  @override
  Future<Object?> call(String method, Map<String, Object?> params) async => jsonDecode(await client.call(method: method, params: jsonEncode(params)));

  /// One stream from the bridge for the profile's lifetime, however many
  /// screens listen.
  @override
  late final Stream<Map<String, Object?>> events = client.eventsJson().map((e) => jsonDecode(e) as Map<String, Object?>).asBroadcastStream();
}
