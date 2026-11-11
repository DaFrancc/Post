#ifndef RUNNER_WINDOW_CHANNEL_H_
#define RUNNER_WINDOW_CHANNEL_H_

#include <flutter_linux/flutter_linux.h>
#include <gtk/gtk.h>

// Postern draws its own window frame. Before the window is shown: remove the
// system decorations and make the surface transparent where the display
// allows it, so the app can draw rounded corners and a shadow.
void postern_window_prepare(GtkWindow* window, FlView* view);

// The "postern/window" method channel the app's top bar and edges use:
// startMove, startResize(edge), minimize, toggleMaximize, close,
// isMaximized, isComposited; and "maximizedChanged" sent to the app.
void postern_window_channel_register(GtkWindow* window, FlView* view);

#endif  // RUNNER_WINDOW_CHANNEL_H_
