/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.loadbalance.exceptions;

import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;

public class NoBackendAvailableException extends LoadBalanceException {

    public NoBackendAvailableException() {
        super();
    }

    public NoBackendAvailableException(String message) {
        super(message);
    }

    public NoBackendAvailableException(String message, Throwable cause) {
        super(message, cause);
    }

    public NoBackendAvailableException(Throwable cause) {
        super(cause);
    }

    protected NoBackendAvailableException(String message, Throwable cause, boolean enableSuppression, boolean writableStackTrace) {
        super(message, cause, enableSuppression, writableStackTrace);
    }
}
