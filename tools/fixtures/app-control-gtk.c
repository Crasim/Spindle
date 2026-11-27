#include <gtk/gtk.h>

static void save_entry(GtkEditable *entry, gpointer path) {
    g_file_set_contents(path, gtk_entry_get_text(GTK_ENTRY(entry)), -1, NULL);
}

int main(int argc, char **argv) {
    if (argc != 2) {
        return 1;
    }
    gtk_init(&argc, &argv);
    GtkWidget *window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(window), "Accessible test editor");
    gtk_window_set_default_size(GTK_WINDOW(window), 480, 220);
    GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
    gtk_container_add(GTK_CONTAINER(window), box);
    GtkWidget *entry = gtk_entry_new();
    atk_object_set_name(gtk_widget_get_accessible(entry), "Fixture field");
    gtk_box_pack_start(GTK_BOX(box), entry, FALSE, FALSE, 8);
    g_signal_connect(entry, "changed", G_CALLBACK(save_entry), argv[1]);
    gtk_widget_show_all(window);
    gtk_widget_grab_focus(entry);
    gtk_main();
    return 0;
}
