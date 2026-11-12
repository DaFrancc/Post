// Runs integration tests under `flutter drive`, which can build in profile
// mode for meaningful timings (tool/timing.sh).

import 'package:integration_test/integration_test_driver.dart';

Future<void> main() => integrationDriver();
