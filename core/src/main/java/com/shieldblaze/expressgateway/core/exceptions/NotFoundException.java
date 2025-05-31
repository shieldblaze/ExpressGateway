package com.shieldblaze.expressgateway.core.exceptions;

import java.io.Serial;

/**
 * <p> Exception to be thrown when a resource is not found. </p>
 */
public final class NotFoundException extends RuntimeException {

    @Serial
    private static final long serialVersionUID = -6677068064053200288L;

    public NotFoundException(Type type) {
        super(type.name() + " not found");
    }

    public NotFoundException(String message) {
        super(message);
    }

    @Override
    public synchronized Throwable fillInStackTrace() {
        return this;
    }

    public enum Type {
        CLUSTER
    }
}
