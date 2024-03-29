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

/**
 * Thrown when there is no {@link Node} available to handle request.
 */
public final class NoNodeAvailableException extends LoadBalanceException {

    public static final NoNodeAvailableException INSTANCE = new NoNodeAvailableException("No Node is available to handle this exception");

    public NoNodeAvailableException() {
        super();
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

    protected NoNodeAvailableException(String message, Throwable cause, boolean enableSuppression, boolean writableStackTrace) {
        super(message, cause, enableSuppression, writableStackTrace);
    }
}
