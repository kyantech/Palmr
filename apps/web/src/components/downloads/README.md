# Sistema de Progresso de Download

Este sistema implementa um toaster de progresso de download similar ao sistema de upload existente.

## Componentes

### `useDownloadManager`
Hook principal que gerencia o estado dos downloads ativos:
- Rastreia progresso em tempo real
- Calcula velocidade de transferência
- Estima tempo restante
- Gerencia cancelamento e retry

### `GlobalDownloadToaster`
Componente visual que exibe os downloads no canto inferior direito:
- Cards individuais para cada download
- Barra de progresso com percentual
- Velocidade e ETA
- Botões de ação (cancelar, retry, remover)

## Funcionalidades

- ✅ Progresso visual em tempo real
- ✅ Cálculo de velocidade de transferência
- ✅ Estimativa de tempo restante (ETA)
- ✅ Cancelamento de downloads
- ✅ Retry para downloads com erro
- ✅ Auto-remoção de downloads concluídos (5s)
- ✅ Suporte a múltiplos downloads simultâneos
- ✅ Traduções em inglês e português
- ✅ Integração com sistema existente

## Como usar

O sistema é automaticamente integrado ao `useFileManager`. Quando um usuário clica em "Download", o arquivo é processado através do `useDownloadManager` e o progresso é exibido no toaster.

### Exemplo de uso direto:

```tsx
import { useDownloadManager } from '@/hooks/use-download-manager';

function MyComponent() {
  const { startDownload } = useDownloadManager();
  
  const handleDownload = async () => {
    await startDownload('object-name', 'filename.pdf');
  };
  
  return <button onClick={handleDownload}>Download</button>;
}
```

## Estados do Download

- **Pending**: Na fila, aguardando início
- **Downloading**: Ativo com barra de progresso
- **Completed**: Concluído com sucesso
- **Error**: Erro com opção de retry
- **Cancelled**: Cancelado pelo usuário

## Arquivos

- `hooks/use-download-manager.ts` - Hook principal
- `components/downloads/global-download-toaster.tsx` - Componente visual
- `components/downloads/download-test.tsx` - Componente de teste
- Traduções adicionadas em `messages/en-US.json` e `messages/pt-BR.json`