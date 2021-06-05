/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

package com.shieldblaze.expressgateway.restapi.response;

public enum ErrorBase {

    // API Errors
    INVALID_REQUEST_DATA(100, "Invalid Request Data"),
    CONFIGURATION_NOT_FOUND(101, "Configuration Not Found");

    private final int ErrorCode;
    private final String ErrorMessage;

    ErrorBase(int ErrorCode, String ErrorMessage) {
        this.ErrorCode = ErrorCode;
        this.ErrorMessage = ErrorMessage;
    }

    public int code() {
        return ErrorCode;
    }

    public String message() {
        return ErrorMessage;
    }
}
