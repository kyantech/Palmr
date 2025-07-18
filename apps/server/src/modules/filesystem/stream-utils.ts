import { Transform } from "stream";

/**
 * Utility functions for safely handling and cleaning up streams
 */

/**
 * Safely close a stream with proper error handling
 * @param stream Stream to close (can be readable, writable, or transform)
 * @param context Optional context for logging
 */
export function safelyCloseStream(
  stream: NodeJS.ReadableStream | NodeJS.WritableStream | Transform | null,
  context?: string
): void {
  if (!stream) return;

  const logContext = context ? ` (${context})` : "";

  try {
    // Remove all listeners to prevent memory leaks
    if ("removeAllListeners" in stream && typeof stream.removeAllListeners === "function") {
      stream.removeAllListeners();
    }

    // Check if stream is already destroyed
    if ("destroyed" in stream && stream.destroyed) {
      console.log(`Stream already destroyed${logContext}`);
      return;
    }

    // Check if stream has destroy method (most Node.js streams do)
    if ("destroy" in stream && typeof stream.destroy === "function") {
      stream.destroy();
      console.log(`Stream destroyed${logContext}`);
    }
    // Fallback to end method for writable streams
    else if ("end" in stream && typeof stream.end === "function") {
      (stream as NodeJS.WritableStream).end();
      console.log(`Stream ended${logContext}`);
    }
    // Fallback to close method if available
    else if ("close" in stream && typeof stream.close === "function") {
      (stream as any).close();
      console.log(`Stream closed${logContext}`);
    }
  } catch (error) {
    console.error(`Error closing stream${logContext}:`, error);
    // Try to force destroy if regular close failed
    try {
      if ("destroy" in stream && typeof stream.destroy === "function") {
        stream.destroy();
      }
    } catch (destroyError) {
      console.error(`Error force destroying stream${logContext}:`, destroyError);
    }
  }
}

/**
 * Safely close multiple streams
 * @param streams Array of streams to close
 * @param context Optional context for logging
 */
export function safelyCloseStreams(
  streams: (NodeJS.ReadableStream | NodeJS.WritableStream | Transform | null)[],
  context?: string
): void {
  streams.forEach((stream, index) => {
    const streamContext = context ? `${context}[${index}]` : `stream[${index}]`;
    safelyCloseStream(stream, streamContext);
  });
}

/**
 * Safely close a stream with timeout
 * @param stream Stream to close
 * @param timeoutMs Timeout in milliseconds (default: 5000)
 * @param context Optional context for logging
 * @returns Promise that resolves when stream is closed or timeout occurs
 */
export function safelyCloseStreamWithTimeout(
  stream: NodeJS.ReadableStream | NodeJS.WritableStream | Transform | null,
  timeoutMs: number = 5000,
  context?: string
): Promise<void> {
  return new Promise((resolve) => {
    if (!stream) {
      resolve();
      return;
    }

    const logContext = context ? ` (${context})` : "";
    let isResolved = false;

    // Set timeout to force close
    const timeout = setTimeout(() => {
      if (!isResolved) {
        console.warn(`Stream close timeout reached${logContext}, force destroying`);
        try {
          if ("destroy" in stream && typeof stream.destroy === "function") {
            stream.destroy();
          }
        } catch (error) {
          console.error(`Error force destroying stream after timeout${logContext}:`, error);
        }
        isResolved = true;
        resolve();
      }
    }, timeoutMs);

    // Listen for close event
    const onClose = () => {
      if (!isResolved) {
        clearTimeout(timeout);
        console.log(`Stream closed gracefully${logContext}`);
        isResolved = true;
        resolve();
      }
    };

    // Listen for error event
    const onError = (error: Error) => {
      if (!isResolved) {
        clearTimeout(timeout);
        console.error(`Stream error during close${logContext}:`, error);
        isResolved = true;
        resolve();
      }
    };

    try {
      // Add event listeners
      if ("on" in stream) {
        stream.on("close", onClose);
        stream.on("error", onError);
      }

      // Attempt to close the stream
      safelyCloseStream(stream, context);
    } catch (error) {
      clearTimeout(timeout);
      console.error(`Error setting up stream close${logContext}:`, error);
      isResolved = true;
      resolve();
    }
  });
}

/**
 * Add error handlers to a stream to prevent uncaught exceptions
 * @param stream Stream to add error handlers to
 * @param context Context string for logging
 */
export function addStreamErrorHandlers(
  stream: NodeJS.ReadableStream | NodeJS.WritableStream | Transform,
  context: string
): void {
  if (!stream) return;

  stream.on("error", (error) => {
    console.error(`Stream error in ${context}:`, error);
  });

  // Handle close events for cleanup
  stream.on("close", () => {
    console.log(`Stream closed in ${context}`);
  });
}

/**
 * Check if a stream is still active/readable
 * @param stream Stream to check
 * @returns True if stream is active
 */
export function isStreamActive(stream: NodeJS.ReadableStream | NodeJS.WritableStream | Transform | null): boolean {
  if (!stream) return false;

  // Check if stream is destroyed
  if ("destroyed" in stream && stream.destroyed) {
    return false;
  }

  // Check if readable stream is still readable
  if ("readable" in stream && typeof stream.readable === "boolean") {
    return stream.readable;
  }

  // Check if writable stream is still writable
  if ("writable" in stream && typeof stream.writable === "boolean") {
    return stream.writable;
  }

  // Default to true if we can't determine the state
  return true;
}

/**
 * Gracefully shutdown a stream pipeline with proper cleanup
 * @param streams Array of streams in the pipeline order
 * @param context Optional context for logging
 * @returns Promise that resolves when all streams are closed
 */
export async function gracefullyShutdownPipeline(
  streams: (NodeJS.ReadableStream | NodeJS.WritableStream | Transform | null)[],
  context?: string
): Promise<void> {
  const logContext = context ? ` (${context})` : "";
  console.log(`Starting graceful shutdown of stream pipeline${logContext}`);

  // Close streams in reverse order (downstream first)
  const reversedStreams = [...streams].reverse();

  for (let i = 0; i < reversedStreams.length; i++) {
    const stream = reversedStreams[i];
    if (stream) {
      const streamContext = context
        ? `${context}[${reversedStreams.length - 1 - i}]`
        : `stream[${reversedStreams.length - 1 - i}]`;
      try {
        await safelyCloseStreamWithTimeout(stream, 3000, streamContext);
      } catch (error) {
        console.error(`Error during graceful shutdown of ${streamContext}:`, error);
      }
    }
  }

  console.log(`Graceful shutdown of stream pipeline completed${logContext}`);
}

/**
 * Clean up stream resources and remove all event listeners
 * @param stream Stream to clean up
 * @param context Optional context for logging
 */
export function cleanupStreamResources(
  stream: NodeJS.ReadableStream | NodeJS.WritableStream | Transform | null,
  context?: string
): void {
  if (!stream) return;

  const logContext = context ? ` (${context})` : "";

  try {
    // Remove all event listeners to prevent memory leaks
    if ("removeAllListeners" in stream && typeof stream.removeAllListeners === "function") {
      stream.removeAllListeners();
      console.log(`Removed all listeners from stream${logContext}`);
    }

    // Unpipe if it's a readable stream
    if ("unpipe" in stream && typeof stream.unpipe === "function") {
      stream.unpipe();
      console.log(`Unpiped stream${logContext}`);
    }

    // Clear any internal buffers if possible
    if ("_readableState" in stream && stream._readableState) {
      const readableState = stream._readableState as any;
      if (readableState.buffer && readableState.buffer.clear) {
        readableState.buffer.clear();
      }
    }

    if ("_writableState" in stream && stream._writableState) {
      const writableState = stream._writableState as any;
      if (writableState.buffer && writableState.buffer.clear) {
        writableState.buffer.clear();
      }
    }
  } catch (error) {
    console.error(`Error cleaning up stream resources${logContext}:`, error);
  }
}

/**
 * Setup automatic cleanup for a stream when it encounters an error or closes
 * @param stream Stream to setup cleanup for
 * @param context Optional context for logging
 * @param onCleanup Optional callback to execute during cleanup
 */
export function setupStreamAutoCleanup(
  stream: NodeJS.ReadableStream | NodeJS.WritableStream | Transform | null,
  context?: string,
  onCleanup?: () => void
): void {
  if (!stream || !("on" in stream)) return;

  const logContext = context ? ` (${context})` : "";

  // Setup error handler
  stream.on("error", (error) => {
    console.error(`Stream error${logContext}:`, error);
    cleanupStreamResources(stream, context);
    if (onCleanup) {
      try {
        onCleanup();
      } catch (cleanupError) {
        console.error(`Error in cleanup callback${logContext}:`, cleanupError);
      }
    }
  });

  // Setup close handler
  stream.on("close", () => {
    console.log(`Stream closed${logContext}`);
    cleanupStreamResources(stream, context);
    if (onCleanup) {
      try {
        onCleanup();
      } catch (cleanupError) {
        console.error(`Error in cleanup callback${logContext}:`, cleanupError);
      }
    }
  });

  // Setup finish handler for writable streams
  if ("writable" in stream && stream.writable) {
    stream.on("finish", () => {
      console.log(`Stream finished${logContext}`);
    });
  }

  // Setup end handler for readable streams
  if ("readable" in stream && stream.readable) {
    stream.on("end", () => {
      console.log(`Stream ended${logContext}`);
    });
  }
}
