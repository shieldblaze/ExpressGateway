package com.shieldblaze.expressgateway.backend.exceptions;

import java.io.Serial;

/**
 * Stackless Exception
 */
public class StacklessException extends RuntimeException {

    @Serial
    private static final long serialVersionUID = 5110097034369377701L;

    public StacklessException() {
    }

    public StacklessException(String message) {
        super(message);
    }

    public StacklessException(String message, Throwable cause) {
        super(message, cause);
    }

    public StacklessException(Throwable cause) {
        super(cause);
    }

    public StacklessException(String message, Throwable cause, boolean enableSuppression, boolean writableStackTrace) {
        super(message, cause, enableSuppression, writableStackTrace);
    }

    @Override
    public synchronized Throwable fillInStackTrace() {
        return this;
    }
}
