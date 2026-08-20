import { SafeAreaProvider } from 'react-native-safe-area-context';

import { AnchorSensorScreen } from './src/screens/AnchorSensorScreen';

function App() {
  return (
    <SafeAreaProvider>
      <AnchorSensorScreen />
    </SafeAreaProvider>
  );
}

export default App;
