# Install Spindle Browser Link

Browser Link connects Spindle desktop to approved tabs in your regular Microsoft Edge or Google Chrome profile on Windows and Linux. In Spindle, ask about a webpage or open **Settings → Computer use** and choose **Set up Microsoft Edge** or **Set up Google Chrome**. Spindle registers its local connection, then verifies and unpacks the extension. It shows the folder in your file manager and opens the guide in your browser. On Windows, the folder is under **Downloads**.

In the browser profile with your tabs, open **Extensions → Manage extensions**, turn on **Developer mode**, and select **Load unpacked**. Choose the Browser Link folder shown in Spindle. To unpack a fresh copy manually, download the Browser Link ZIP from the setup guide or this project's GitHub Releases and extract it into Downloads. Choose the extracted folder containing `manifest.json`. Spindle confirms the connection and continues any waiting chat. [Edge setup](https://learn.microsoft.com/en-us/microsoft-edge/extensions/getting-started/extension-sideloading) · [Chrome setup](https://developer.chrome.com/docs/extensions/get-started/tutorial/hello-world#load-unpacked)

Use the chat’s **Allow this site**, **Allow all**, or **Deny** choices when it asks for website access. Browser Link is separate from the isolated browser inside Spindle.

## Publishing

When extension files change, update the version in `manifest.json` and the download link in the main README. Tag the version as `browser-link-v<VERSION>` and push the tag. GitHub Actions publishes the extension ZIP on that release.
