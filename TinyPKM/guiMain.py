import tkinter as tk
from tkinter.ttk import *
from tkinter import *
from tkinter import PhotoImage

# Create the main window
mainWindow = tk.Tk()
mainWindow.title("Matt's PKspace")
mainWindow.geometry("1920x1080")
mainWindow.configure(bg="gray19")

# Add widgets (e.g., a label)
label = tk.Label(mainWindow, text="test")
window_icon = PhotoImage(file = "E:\Desktop\Code\VScode workspace\python\PKspace\icons\plus.png") #main window icon
label.pack()

#main window icon
mainWindow.iconphoto(True, window_icon)

#define event to fullscreen
def togfullscr(event = None):
   mainWindow.attributes('-fullscreen', not mainWindow.attributes('-fullscreen'))

#define event to close window
def close_win(e):
   mainWindow.destroy()

# Bind F to fullscreen with callback function
mainWindow.bind('f', togfullscr)

# Bind the ESC key with the callback function
mainWindow.bind('<Escape>', lambda e: close_win(e))

# Start the event loop
mainWindow.mainloop()
