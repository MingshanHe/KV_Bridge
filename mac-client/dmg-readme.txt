kvm-mac: lets the Windows laptop's keyboard and mouse control this Mac.

1. Drag the "kvm-mac" folder from this window onto your Desktop
   (or into Documents). It won't run from inside this disk image.

2. Open the copied folder, right-click "Start kvm-mac" and choose Open.
   If macOS says it can't be opened: System Settings > Privacy & Security,
   scroll down, click "Open Anyway", then try again.

3. Type the IP address that kvm-win shows on the Windows laptop. It is
   remembered for next time.

4. The first time, macOS asks to give Terminal "Accessibility" access.
   Turn it on (System Settings > Privacy & Security > Accessibility),
   then start kvm-mac again.

5. Press the Lift's Forward button on Windows to control the Mac;
   press it again to go back.

Test without Windows: in Terminal, run the copied kvm-mac with --test-type,
then click into TextEdit. It should type "hello from kvm bridge".
