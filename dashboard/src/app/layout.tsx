export const metadata = {
  title: "Labrys operator console",
  description: "Platform-owned application truth for Labrys operators",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>
        <a href="#main">Skip to content</a>
        <nav aria-label="operator sections">
          <ul>
            <li><a href="/">Overview</a></li>
            <li><a href="/deployments">Deployments</a></li>
            <li><a href="/approvals">Approvals</a></li>
            <li><a href="/secrets">Secrets</a></li>
          </ul>
        </nav>
        <div id="main">{children}</div>
      </body>
    </html>
  );
}
