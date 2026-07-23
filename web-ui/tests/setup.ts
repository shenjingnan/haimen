import '@testing-library/jest-dom/vitest'

// shadcn/ui 组件依赖 ResizeObserver，jsdom 未实现
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
window.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver
