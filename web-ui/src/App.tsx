import { Button, ConfigProvider, theme } from 'antd'

export default function App() {
  return (
    <ConfigProvider
      theme={{
        algorithm: theme.defaultAlgorithm,
      }}
    >
      <div className="flex items-center justify-center min-h-screen">
        <Button type="primary">Ant Design Button</Button>
      </div>
    </ConfigProvider>
  )
}
