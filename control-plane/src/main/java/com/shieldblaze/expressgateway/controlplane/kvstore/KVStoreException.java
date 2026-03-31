/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.controlplane.kvstore;

/**
 * Exception thrown by {@link KVStore} operations, carrying a structured {@link Code}
 * that allows callers to distinguish between retriable and permanent failures.
 */
public class KVStoreException extends Exception {

    /**
     * Error codes representing categories of KV store failures.
     */
    public enum Code {
        /** The connection to the backend was lost (retriable). */
        CONNECTION_LOST,

        /** A CAS operation failed because the expected version did not match. */
        VERSION_CONFLICT,

        /** The requested key does not exist. */
        KEY_NOT_FOUND,

        /** The operation exceeded its deadline. */
        OPERATION_TIMEOUT,

        /** The caller lacks permission for this operation. */
        UNAUTHORIZED,

        /** An unclassified internal error occurred. */
        INTERNAL_ERROR
    }

    private final Code code;

    public KVStoreException(Code code, String message) {
        super(message);
        this.code = code;
    }

    public KVStoreException(Code code, String message, Throwable cause) {
        super(message, cause);
        this.code = code;
    }

    /**
     * Returns the structured error code for this exception.
     */
    public Code code() {
        return code;
    }
}
