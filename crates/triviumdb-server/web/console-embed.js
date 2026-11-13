/* 同源 iframe 挂载入口：页面拥有 DOM，宿主拥有会话、数据库和关闭顺序。 */
(function (root) {
  'use strict';
  root.TriviumConsoleEmbed = Object.freeze({
    async mount(container, options) {
      if (!container || !options || typeof options.storageKey !== 'string' || !options.storageKey.trim()) {
        throw new Error('需要容器和稳定的数据库存储标识 (Container and stable database storage key required)');
      }
      if (typeof options.detach !== 'function') {
        throw new Error('需要宿主 detach 回调 (Host detach callback required)');
      }
      const url = new URL(options.url, location.href);
      if (url.origin !== location.origin) throw new Error('控制台资源必须同源 (Console assets must share the host origin)');
      url.searchParams.delete('selftest');
      url.searchParams.set('embedded', '1');
      url.searchParams.set('storageKey', options.storageKey);
      const frame = document.createElement('iframe');
      frame.title = options.title || 'TriviumDB Console';
      frame.style.cssText = 'width:100%;height:100%;border:0';
      let detaching = null;
      const detach = () => {
        if (!detaching) detaching = Promise.resolve().then(() => options.detach()).catch(error => {
          detaching = null;
          throw error;
        });
        return detaching;
      };
      const loaded = new Promise((resolve, reject) => {
        frame.addEventListener('load', resolve, { once: true });
        frame.addEventListener('error', () => reject(new Error('控制台资源加载失败 (Console assets failed to load)')), { once: true });
      });
      frame.src = url.href;
      container.appendChild(frame);
      let api;
      try {
        await loaded;
        api = frame.contentWindow.TriviumConsole;
        if (!api) throw new Error('控制台挂载入口缺失 (Console mount entry missing)');
        const transport = options.request
          ? { request: options.request, cancel: options.cancel, detach }
          : frame.contentWindow.TriviumConsoleClient.httpTransport({
            endpoint: new URL(options.endpoint || '/v1/console', location.href).href,
            headers: options.headers, credentials: options.credentials, detach,
          });
        const info = await api.mount(transport);
        return Object.freeze({
          frame, info,
          pendingTransaction: () => api.pendingTransaction(),
          async dispose(disposeOptions) {
            // 草稿保护拒绝卸载时保留 iframe，供调用方继续导出或确认。
            await api.dispose(disposeOptions);
            frame.remove();
          },
        });
      } catch (error) {
        try { await detach(); } finally { frame.remove(); }
        throw error;
      }
    },
  });
})(globalThis);
