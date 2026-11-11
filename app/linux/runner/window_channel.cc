// Post's own window frame on Linux (X11 and Wayland).
//
// Moving and resizing are handed to the window manager from the mouse press
// that started them: compositors, Wayland ones above all, accept a move or
// resize only for a real press, so the press is recorded as it arrives and
// its button, position and time are used. Once the window manager takes the
// pointer it swallows the matching release, and Flutter would think the
// button is still held: hover, clicks and cursors would all stop working.
// So a release is delivered to Flutter straight after handing over.

#include "window_channel.h"

namespace {

struct WindowChannel {
  GtkWindow* window = nullptr;
  FlMethodChannel* channel = nullptr;
  GdkEvent* last_press = nullptr;  // copy of the latest button press
};

WindowChannel* g_state = nullptr;

// Sees every event before GTK dispatches it, to remember the latest press.
void record_events(GdkEvent* event, gpointer) {
  if (g_state != nullptr && event->type == GDK_BUTTON_PRESS) {
    if (g_state->last_press != nullptr) gdk_event_free(g_state->last_press);
    g_state->last_press = gdk_event_copy(event);
  }
  gtk_main_do_event(event);
}

// Tell Flutter the press is over: the window manager now owns the pointer
// and will not deliver the real release.
void release_to_flutter() {
  if (g_state->last_press == nullptr) return;
  GdkEvent* release = gdk_event_copy(g_state->last_press);
  release->type = GDK_BUTTON_RELEASE;
  release->button.send_event = TRUE;
  gdk_event_put(release);
  gdk_event_free(release);
}

GdkEventButton* press() {
  return g_state->last_press != nullptr ? &g_state->last_press->button : nullptr;
}

void start_move() {
  GdkEventButton* p = press();
  if (p == nullptr) return;
  gtk_window_begin_move_drag(g_state->window, p->button, static_cast<gint>(p->x_root),
                             static_cast<gint>(p->y_root), p->time);
  release_to_flutter();
}

void start_resize(GdkWindowEdge edge) {
  GdkEventButton* p = press();
  if (p == nullptr) return;
  gtk_window_begin_resize_drag(g_state->window, edge, p->button, static_cast<gint>(p->x_root),
                               static_cast<gint>(p->y_root), p->time);
  release_to_flutter();
}

bool is_maximized() { return gtk_window_is_maximized(g_state->window); }

bool is_composited() {
  GdkScreen* screen = gtk_window_get_screen(g_state->window);
  return screen != nullptr && gdk_screen_is_composited(screen) &&
         gdk_screen_get_rgba_visual(screen) != nullptr;
}

FlMethodResponse* bool_response(bool value) {
  g_autoptr(FlValue) v = fl_value_new_bool(value);
  return FL_METHOD_RESPONSE(fl_method_success_response_new(v));
}

FlMethodResponse* done() { return FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr)); }

void on_method_call(FlMethodChannel* channel, FlMethodCall* call, gpointer) {
  const gchar* method = fl_method_call_get_name(call);
  g_autoptr(FlMethodResponse) response = nullptr;
  if (g_strcmp0(method, "startMove") == 0) {
    start_move();
    response = done();
  } else if (g_strcmp0(method, "startResize") == 0) {
    FlValue* args = fl_method_call_get_args(call);
    if (args == nullptr || fl_value_get_type(args) != FL_VALUE_TYPE_INT) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new("bad-edge", "edge must be an integer", nullptr));
    } else {
      int64_t edge = fl_value_get_int(args);
      if (edge < GDK_WINDOW_EDGE_NORTH_WEST || edge > GDK_WINDOW_EDGE_SOUTH_EAST) {
        response = FL_METHOD_RESPONSE(fl_method_error_response_new("bad-edge", "no such edge", nullptr));
      } else {
        start_resize(static_cast<GdkWindowEdge>(edge));
        response = done();
      }
    }
  } else if (g_strcmp0(method, "minimize") == 0) {
    gtk_window_iconify(g_state->window);
    response = done();
  } else if (g_strcmp0(method, "toggleMaximize") == 0) {
    if (is_maximized()) {
      gtk_window_unmaximize(g_state->window);
    } else {
      gtk_window_maximize(g_state->window);
    }
    response = done();
  } else if (g_strcmp0(method, "close") == 0) {
    gtk_window_close(g_state->window);
    response = done();
  } else if (g_strcmp0(method, "isMaximized") == 0) {
    response = bool_response(is_maximized());
  } else if (g_strcmp0(method, "isComposited") == 0) {
    response = bool_response(is_composited());
  } else {
    response = FL_METHOD_RESPONSE(fl_method_not_implemented_response_new());
  }
  g_autoptr(GError) error = nullptr;
  if (!fl_method_call_respond(call, response, &error)) {
    g_warning("post/window: could not respond: %s", error->message);
  }
}

gboolean on_window_state(GtkWidget*, GdkEventWindowState* event, gpointer) {
  if ((event->changed_mask & GDK_WINDOW_STATE_MAXIMIZED) != 0 && g_state->channel != nullptr) {
    g_autoptr(FlValue) v = fl_value_new_bool((event->new_window_state & GDK_WINDOW_STATE_MAXIMIZED) != 0);
    fl_method_channel_invoke_method(g_state->channel, "maximizedChanged", v, nullptr, nullptr, nullptr);
  }
  return FALSE;
}

}  // namespace

void post_window_prepare(GtkWindow* window, FlView* view) {
  // Client-side decorations, so GTK tells the window manager the window
  // draws its own frame (KDE's decoration protocol on Wayland, Motif hints
  // on X11). Merely turning decorations off does the opposite on KDE: GTK
  // then asks KWin to draw its frame around the app's. The title bar widget
  // that switches GTK into this mode is never shown; the app draws its own.
  GtkWidget* titlebar = gtk_drawing_area_new();
  gtk_window_set_titlebar(window, titlebar);

  GdkScreen* screen = gtk_window_get_screen(window);
  // GTK would draw its own shadow, border and resize margin around a
  // client-decorated window; the app draws those itself.
  g_autoptr(GtkCssProvider) css = gtk_css_provider_new();
  gtk_css_provider_load_from_data(css,
                                  "decoration { box-shadow: none; border: none; margin: 0; padding: 0;"
                                  " border-radius: 0; background: none; }"
                                  "window, window.csd, window.background { background: transparent;"
                                  " box-shadow: none; border: none; }",
                                  -1, nullptr);
  if (screen != nullptr) {
    gtk_style_context_add_provider_for_screen(screen, GTK_STYLE_PROVIDER(css),
                                              GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
  }
  GdkVisual* rgba = screen != nullptr ? gdk_screen_get_rgba_visual(screen) : nullptr;
  if (rgba != nullptr && gdk_screen_is_composited(screen)) {
    // Transparent outside the app's rounded frame.
    gtk_widget_set_visual(GTK_WIDGET(window), rgba);
    gtk_widget_set_app_paintable(GTK_WIDGET(window), TRUE);
    GdkRGBA clear = {0, 0, 0, 0};
    fl_view_set_background_color(view, &clear);
  } else {
    GdkRGBA ground;
    gdk_rgba_parse(&ground, "#06070C");
    fl_view_set_background_color(view, &ground);
  }
  // The app's own minimum, plus the shadow margin it draws around itself.
  GdkGeometry hints = {};
  hints.min_width = 880 + 2 * 12;
  hints.min_height = 580 + 2 * 12;
  gtk_window_set_geometry_hints(window, nullptr, &hints, GDK_HINT_MIN_SIZE);
}

void post_window_channel_register(GtkWindow* window, FlView* view) {
  g_state = new WindowChannel();
  g_state->window = window;
  gdk_event_handler_set(record_events, nullptr, nullptr);

  FlEngine* engine = fl_view_get_engine(view);
  g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
  g_state->channel = fl_method_channel_new(fl_engine_get_binary_messenger(engine), "post/window",
                                           FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(g_state->channel, on_method_call, nullptr, nullptr);
  g_signal_connect(window, "window-state-event", G_CALLBACK(on_window_state), nullptr);
}
