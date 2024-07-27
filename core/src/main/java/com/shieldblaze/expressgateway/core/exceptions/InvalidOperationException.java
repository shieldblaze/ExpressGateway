package com.shieldblaze.expressgateway.core.exceptions;

import java.io.Serial;

public final class InvalidOperationException extends RuntimeException {

    @Serial
    private static final long serialVersionUID = -4203302259240123822L;

    public InvalidOperationException(String message) {
        super(message);
    }

    public InvalidOperationException(String message, Throwable cause) {
        super(message, cause);
    }

    @Override
    public synchronized Throwable fillInStackTrace() {
        return this;
    }
}
