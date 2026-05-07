Output demo.gif
Set Shell "bash"
Set FontSize 18
Set Width 1200
Set Height 700

Type "bat --theme ansi --no-pager example/add.c example/a.c example/b.c"
Enter
Sleep 3

Type "make -C example"
Enter
Sleep 1

Type "cgdiff example/a.so example/b.so"
Enter
Sleep 2

Enter
Sleep 3
Escape
Sleep 1
Escape
