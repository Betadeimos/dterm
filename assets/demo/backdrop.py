"""A borderless full-screen window showing backdrop.png, for dterm's acrylic to blur."""
import ctypes, os, tempfile, tkinter as tk

ctypes.windll.shcore.SetProcessDpiAwareness(2)
root = tk.Tk()
root.overrideredirect(True)
root.attributes("-topmost", True)  # over every normal window; dterm goes topmost after it
root.geometry(f"{root.winfo_screenwidth()}x{root.winfo_screenheight()}+0+0")
img = tk.PhotoImage(file=os.path.join(tempfile.gettempdir(), "dterm-demo", "backdrop.png"))  # made by world.py
tk.Label(root, image=img, borderwidth=0).pack()
root.mainloop()
