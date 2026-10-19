// Visual readiness only. This never signals payment or fulfillment success.
(() => {
  if (window.top !== window) return;
  let checks = 0;
  const visibleSquare = (element, width, height) => {
    const box = element.getBoundingClientRect();
    return width >= 100 && height >= 100 && Math.abs(width / height - 1) < 0.15
      && box.width >= 100 && box.height >= 100;
  };
  const center = (qr) => {
    const root = document.documentElement;
    const body = document.body;
    root.style.setProperty('overflow', 'hidden', 'important');
    const original = window.getComputedStyle(body).transform;
    let x = 0, y = 0;
    const align = () => {
      const box = qr.getBoundingClientRect();
      x += (window.innerWidth - box.width) / 2 - box.left;
      y += (window.innerHeight - box.height) / 2 - box.top;
      // Move the complete provider content together so the Alipay logo and
      // any overlays keep their original position within the QR. Do not
      // copy, crop, stretch or regenerate payment content.
      body.style.setProperty('transform', `translate(${x}px, ${y}px)${original === 'none' ? '' : ` ${original}`}`, 'important');
    };
    align();
    window.addEventListener('resize', align);
  };
  const timer = setInterval(() => {
    const image = Array.from(document.images).find((item) => item.complete
      && visibleSquare(item, item.naturalWidth, item.naturalHeight));
    const canvas = Array.from(document.querySelectorAll('canvas')).find((item) =>
      visibleSquare(item, item.width, item.height));
    if (image || canvas) {
      clearInterval(timer);
      center(image || canvas);
      window.ipc?.postMessage('qr-content-ready');
    } else if (++checks >= 120) clearInterval(timer);
  }, 200);
})();
