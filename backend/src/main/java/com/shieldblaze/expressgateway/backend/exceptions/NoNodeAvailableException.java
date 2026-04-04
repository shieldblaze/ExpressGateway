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
package com.shieldblaze.expressgateway.backend.exceptions;

import com.shieldblaze.expressgateway.backend.Node;

import java.io.Serial;

/**
 * Thrown when there is no {@link Node} available to handle request.
 */
public final class NoNodeAvailableException extends LoadBalanceException {

    @Serial
    private static final long serialVersionUID = 3016237192356488630L;

    /**
     * Shared stackless instance for hot-path use. Because this instance has no stack
     * trace ({@code writableStackTrace=false}), it is safe to rethrow from any context
     * without leaking a stale throw location.
     */
    public static final NoNodeAvailableException INSTANCE = new NoNodeAvailableException(
            "No Node is available to handle this request", null, true, false);

    public NoNodeAvailableException() {
    }

    public NoNodeAvailableException(String message) {
        super(message);
    }

    public NoNodeAvailableException(String message, Throwable cause) {
        super(message, cause);
    }

    public NoNodeAvailableException(Throwable cause) {
        super(cause);
    }

    private NoNodeAvailableException(String message, Throwable cause, boolean enableSuppression, boolean writableStackTrace) {
        super(message, cause, enableSuppression, writableStackTrace);
    }
}
