/*
 * Placeholder Stylos shell.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * WP1 only proves that the image boots, Cage owns the screen and one GTK4
 * client runs there. WP2 replaces this with the real editor.
 *
 * Deliberately not a terminal emulator and it never touches the network.
 */
#include <gtk/gtk.h>

static void activate(GtkApplication *app, gpointer user_data)
{
    (void)user_data;

    GtkWidget *window = gtk_application_window_new(app);
    gtk_window_set_title(GTK_WINDOW(window), "Stylos");
    gtk_window_set_default_size(GTK_WINDOW(window), 1024, 768);
    gtk_window_fullscreen(GTK_WINDOW(window));

    GtkWidget *label = gtk_label_new(NULL);
    gtk_label_set_markup(GTK_LABEL(label),
                         "<span size='96000' weight='bold'>Stylos</span>");
    gtk_widget_set_halign(label, GTK_ALIGN_CENTER);
    gtk_widget_set_valign(label, GTK_ALIGN_CENTER);
    gtk_window_set_child(GTK_WINDOW(window), label);

    GtkCssProvider *css = gtk_css_provider_new();
    gtk_css_provider_load_from_string(css,
        "window { background: #f7f3ea; }"
        "label { color: #24211c; }");
    gtk_style_context_add_provider_for_display(
        gdk_display_get_default(), GTK_STYLE_PROVIDER(css),
        GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
    g_object_unref(css);

    gtk_window_present(GTK_WINDOW(window));
}

int main(int argc, char **argv)
{
    GtkApplication *app = gtk_application_new("app.stylos.Shell",
                                              G_APPLICATION_DEFAULT_FLAGS);
    g_signal_connect(app, "activate", G_CALLBACK(activate), NULL);
    int status = g_application_run(G_APPLICATION(app), argc, argv);
    g_object_unref(app);
    return status;
}
