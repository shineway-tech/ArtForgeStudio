// Visual readiness only. This never signals payment or fulfillment success.
(() => {
  if (window.top !== window) return;
  let checks = 0;
  const visibleSquare = (element, width, height) => {
    const box = element.getBoundingClientRect();
    return width >= 100 && height >= 100 && Math.abs(width / height - 1) < 0.15
      && box.width >= 100 && box.height >= 100;
  };
  const timer = setInterval(() => {
    const image = Array.from(document.images).some((item) => item.complete
      && visibleSquare(item, item.naturalWidth, item.naturalHeight));
    const canvas = Array.from(document.querySelectorAll('canvas')).some((item) =>
      visibleSquare(item, item.width, item.height));
    if (image || canvas) {
      clearInterval(timer);
      window.ipc?.postMessage('qr-content-ready');
    } else if (++checks >= 120) clearInterval(timer);
  }, 200);
})();
