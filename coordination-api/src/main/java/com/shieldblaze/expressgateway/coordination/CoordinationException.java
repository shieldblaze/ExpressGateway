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
package com.shieldblaze.expressgateway.coordination;

import lombok.Getter;

/**
 * Structured exception for all coordination operations. Carries an error {@link Code}
 * that allows callers to distinguish between retriable failures (e.g. {@link Code#CONNECTION_LOST})
 * and permanent failures (e.g. {@link Code#KEY_NOT_FOUND}).
 *
 * <p>Every backend implementation MUST map its native exceptions to this hierarchy.
 * Callers MUST NOT inspect the cause chain for backend-specific exception types.</p>
 */
@Getter
public class CoordinationException extends Exception {

    /**
     * Error codes representing categories of coordination failures.
     */
    public enum Code {
        /** The connection to the backend was lost or is unavailable (retriable). */
        CONNECTION_LOST,

        /** A CAS operation failed because the expected version did not match. */
        VERSION_CONFLICT,

        /** The requested key does not exist. */
        KEY_NOT_FOUND,

        /** The key already exists (e.g. create-if-absent with CAS version 0). */
        KEY_EXISTS,

        /** The operation exceeded its deadline. */
        OPERATION_TIMEOUT,

        /** A distributed lock could not be acquired within the requested timeout. */
        LOCK_ACQUISITION_FAILED,

        /** The operation requires leadership but this node is not the leader. */
        NOT_LEADER,

        /** An unclassified internal error occurred. */
        INTERNAL_ERROR
    }

    private final Code code;

    public CoordinationException(Code code, String message) {
        super(message);
        this.code = code;
    }

    public CoordinationException(Code code, String message, Throwable cause) {
        super(message, cause);
        this.code = code;
    }

    @Override
    public String toString() {
        return "CoordinationException{code=" + code + ", message=" + getMessage() + "}";
    }
}
