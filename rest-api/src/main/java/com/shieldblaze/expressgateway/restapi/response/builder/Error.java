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

public class Error {

    private final int ErrorCode;
    private final String ErrorMessage;

    private Error(ErrorBuilder errorBuilder) {
        ErrorCode = errorBuilder.ErrorCode;
        ErrorMessage = errorBuilder.ErrorMessage;
    }

    /**
     * Create a new ErrorBuilder
     *
     * @return ErrorBuilder
     */
    public static ErrorBuilder newBuilder() {
        return new ErrorBuilder();
    }

    public int getErrorCode() {
        return ErrorCode;
    }

    public String getErrorMessage() {
        return ErrorMessage;
    }

    public static final class ErrorBuilder {
        private int ErrorCode;
        private String ErrorMessage;
        private ErrorBase errorBase = null;

        public ErrorBuilder withErrorBase(ErrorBase errorBase) {
            this.errorBase = errorBase;
            return this;
        }

        public Error build() {

            if (errorBase != null) {
                this.ErrorCode = errorBase.getErrorCode();
                this.ErrorMessage = errorBase.getErrorMessage();
            }

            return new Error(this);
        }
    }

}
