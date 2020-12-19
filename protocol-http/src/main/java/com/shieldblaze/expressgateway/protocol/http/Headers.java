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
package com.shieldblaze.expressgateway.protocol.http;

public final class Headers {

    /**
     * "x-forwarded-for"
     */
    public static final String X_FORWARDED_FOR = "x-forwarded-for";

    /**
     * x-forwarded-http-version
     */
    public static final String X_FORWARDED_HTTP_VERSION = "x-forwarded-http-version";

    public static final class Values {

        /**
         * http_1_0
         */
        public static final String HTTP_1_0 = "http_1_0";

        /**
         * http_1_1
         */
        public static final String HTTP_1_1 = "http_1_1";

        /**
         * h2
         */
        public static final String HTTP_2 = "h2";
    }
}
