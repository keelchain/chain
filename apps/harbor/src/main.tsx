import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { BrowserRouter } from 'react-router-dom';
import { App } from './App';
import { ErrorBoundary } from './components/Layout';
import './styles.css';

const base = (import.meta.env.BASE_URL ?? '/').replace(/\/$/, '');

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <ErrorBoundary>
      <BrowserRouter basename={base || undefined}>
        <App />
      </BrowserRouter>
    </ErrorBoundary>
  </StrictMode>,
);
