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
package com.shieldblaze.expressgateway.core.server.http;

import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http.HttpServerCodec;

final class HTTPCodecs {

    /**
     * Create new {@link HttpServerCodec} Instance
     * @param httpConfiguration {@link HTTPConfiguration} Instance
     */
    static HttpServerCodec newServer(HTTPConfiguration httpConfiguration) {
        return new HttpServerCodec(httpConfiguration.getMaxInitialLineLength(), httpConfiguration.getMaxHeaderSize(),
                httpConfiguration.getMaxChunkSize(), true);
    }

    /**
     * Create new {@link HttpClientCodec} Instance
     * @param httpConfiguration {@link HTTPConfiguration} Instance
     */
    static HttpClientCodec newClient(HTTPConfiguration httpConfiguration) {
        return new HttpClientCodec(httpConfiguration.getMaxInitialLineLength(), httpConfiguration.getMaxHeaderSize(),
                httpConfiguration.getMaxChunkSize(), true);
    }
}
