// The page a QR code from Settings > Devices opens on the new device: the
// code rides in the fragment, which never reaches the box, and is shown
// here to type into the app.
const code = decodeURIComponent(location.hash.slice(1)).toUpperCase();
if (/^[2-9A-Z]{3}-[2-9A-Z]{3}$/.test(code)) {
  document.getElementById('code').textContent = code;
  document.getElementById('code').hidden = false;
  document.getElementById('with-code').hidden = false;
  document.getElementById('without-code').hidden = true;
}
