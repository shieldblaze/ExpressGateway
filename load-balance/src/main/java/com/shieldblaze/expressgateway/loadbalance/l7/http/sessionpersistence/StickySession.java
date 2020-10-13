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
package com.shieldblaze.expressgateway.loadbalance.l7.http.sessionpersistence;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.Request;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPRequest;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPResponse;

public final class StickySession implements SessionPersistence<Backend, HTTPResponse, Backend> {

    @Override
    public Backend getBackend(Request request) {
        return getBackend((HTTPRequest) request);
    }

    public Backend getBackend(HTTPRequest httpRequest) {
        return null;
    }

    @Override
    public void addRoute(HTTPResponse key, Backend value) {

    }
}
