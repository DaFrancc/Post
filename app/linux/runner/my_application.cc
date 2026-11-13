#include "my_application.h"

#include <flutter_linux/flutter_linux.h>
#ifdef GDK_WINDOWING_WAYLAND
#include <gdk/gdkwayland.h>
#endif

#include "flutter/generated_plugin_registrant.h"
#include "window_channel.h"

struct _MyApplication {
  GtkApplication parent_instance;
  char** dart_entrypoint_arguments;
};

G_DEFINE_TYPE(MyApplication, my_application, GTK_TYPE_APPLICATION)

// On Wayland, GTK can paint the texture Flutter shares with it before the
// newest frame has landed in it, so the window shows the frame before:
// one frame behind. Usually another frame follows and hides it; when
// nothing else moves (the pointer stops on a menu item, say), the window
// keeps the old picture until the next frame, as if hover lagged behind
// the pointer. Seen with Flutter 3.47 on KDE Plasma with NVIDIA; the
// engine does not wait for the frame before GTK reads it
// (flutter/flutter#195821, and the new Wayland renderer in later Flutter
// replaces this path). Until the engine waits: after each paint, paint
// once more on the next frame, by when the frame is complete. One extra
// paint at the end of each burst of frames; Flutter's next frame and this
// paint merge when it is busy. Remove once Flutter is upgraded past it.
static gboolean repaint_pending = FALSE;
static void repaint_once_more(GdkFrameClock*, gpointer view) {
  if (repaint_pending) {
    repaint_pending = FALSE;  // This was the extra paint.
    return;
  }
  repaint_pending = TRUE;
  gtk_widget_queue_draw(GTK_WIDGET(view));
}

// Called when first Flutter frame received.
static void first_frame_cb(MyApplication* self, FlView* view) {
  gtk_widget_show(gtk_widget_get_toplevel(GTK_WIDGET(view)));
}

// Implements GApplication::activate.
static void my_application_activate(GApplication* application) {
  MyApplication* self = MY_APPLICATION(application);
  GtkWindow* window =
      GTK_WINDOW(gtk_application_window_new(GTK_APPLICATION(application)));

  // Post draws its own top bar (window_channel.cc), so no header bar or
  // system title bar. The title still names the window to the desktop.
  gtk_window_set_title(window, "Post");
  // The icon from the installed theme (linux/packaging). Used on X11; on
  // Wayland the desktop takes it from the desktop entry for APPLICATION_ID.
  gtk_window_set_icon_name(window, APPLICATION_ID);
  gtk_window_set_default_size(window, 1280 + 2 * 12, 800 + 2 * 12);

  g_autoptr(FlDartProject) project = fl_dart_project_new();
  fl_dart_project_set_dart_entrypoint_arguments(
      project, self->dart_entrypoint_arguments);

  FlView* view = fl_view_new(project);
  post_window_prepare(window, view);
  gtk_widget_show(GTK_WIDGET(view));
  gtk_container_add(GTK_CONTAINER(window), GTK_WIDGET(view));

  // Show the window when Flutter renders.
  // Requires the view to be realized so we can start rendering.
  g_signal_connect_swapped(view, "first-frame", G_CALLBACK(first_frame_cb),
                           self);
  gtk_widget_realize(GTK_WIDGET(view));

#ifdef GDK_WINDOWING_WAYLAND
  // The window one frame behind on Wayland: see repaint_once_more. X11
  // does not need it.
  if (GDK_IS_WAYLAND_DISPLAY(gtk_widget_get_display(GTK_WIDGET(window)))) {
    GdkFrameClock* clock = gtk_widget_get_frame_clock(GTK_WIDGET(window));
    if (clock != nullptr) {
      g_signal_connect(clock, "after-paint", G_CALLBACK(repaint_once_more), view);
    }
  }
#endif

  fl_register_plugins(FL_PLUGIN_REGISTRY(view));
  post_window_channel_register(window, view);

  gtk_widget_grab_focus(GTK_WIDGET(view));
}

// Implements GApplication::local_command_line.
static gboolean my_application_local_command_line(GApplication* application,
                                                  gchar*** arguments,
                                                  int* exit_status) {
  MyApplication* self = MY_APPLICATION(application);
  // Strip out the first argument as it is the binary name.
  self->dart_entrypoint_arguments = g_strdupv(*arguments + 1);

  g_autoptr(GError) error = nullptr;
  if (!g_application_register(application, nullptr, &error)) {
    g_warning("Failed to register: %s", error->message);
    *exit_status = 1;
    return TRUE;
  }

  g_application_activate(application);
  *exit_status = 0;

  return TRUE;
}

// Implements GApplication::startup.
static void my_application_startup(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application startup.

  G_APPLICATION_CLASS(my_application_parent_class)->startup(application);
}

// Implements GApplication::shutdown.
static void my_application_shutdown(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application shutdown.

  G_APPLICATION_CLASS(my_application_parent_class)->shutdown(application);
}

// Implements GObject::dispose.
static void my_application_dispose(GObject* object) {
  MyApplication* self = MY_APPLICATION(object);
  g_clear_pointer(&self->dart_entrypoint_arguments, g_strfreev);
  G_OBJECT_CLASS(my_application_parent_class)->dispose(object);
}

static void my_application_class_init(MyApplicationClass* klass) {
  G_APPLICATION_CLASS(klass)->activate = my_application_activate;
  G_APPLICATION_CLASS(klass)->local_command_line =
      my_application_local_command_line;
  G_APPLICATION_CLASS(klass)->startup = my_application_startup;
  G_APPLICATION_CLASS(klass)->shutdown = my_application_shutdown;
  G_OBJECT_CLASS(klass)->dispose = my_application_dispose;
}

static void my_application_init(MyApplication* self) {}

MyApplication* my_application_new() {
  // Set the program name to the application ID, which helps various systems
  // like GTK and desktop environments map this running application to its
  // corresponding .desktop file. This ensures better integration by allowing
  // the application to be recognized beyond its binary name.
  g_set_prgname(APPLICATION_ID);

  return MY_APPLICATION(g_object_new(my_application_get_type(),
                                     "application-id", APPLICATION_ID, "flags",
                                     G_APPLICATION_NON_UNIQUE, nullptr));
}
