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
package com.shieldblaze.expressgateway.backend.connection;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.exceptions.BackendNotAvailableException;

public class TooManyConnectionsException extends BackendNotAvailableException {

    public TooManyConnectionsException(Backend backend) {
        super("Backend: " + backend.getSocketAddress() + " has too many active connections. Increase Maximum Connection limit to create new connection.");
    }
}
