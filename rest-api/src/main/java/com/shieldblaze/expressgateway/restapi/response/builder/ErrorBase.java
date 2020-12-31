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

package com.shieldblaze.expressgateway.restapi.response.builder;

public enum ErrorBase {

    // API Errors
    INTERNAL_SERVER_ERROR(100, "Internal Server Error"),
    REQUEST_ERROR(101, "Request Error"),


    LOADBALANCER_NOT_FOUND(200, "LoadBalancer not found")

    ;

    private final int ErrorCode;
    private final String ErrorMessage;

    ErrorBase(int ErrorCode, String ErrorMessage) {
        this.ErrorCode = ErrorCode;
        this.ErrorMessage = ErrorMessage;
    }

    public int getErrorCode() {
        return ErrorCode;
    }

    public String getErrorMessage() {
        return ErrorMessage;
    }
}
