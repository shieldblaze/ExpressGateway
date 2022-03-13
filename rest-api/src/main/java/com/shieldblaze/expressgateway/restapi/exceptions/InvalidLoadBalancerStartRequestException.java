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
package com.shieldblaze.expressgateway.restapi.exceptions;

import com.shieldblaze.expressgateway.restapi.api.loadbalancer.LoadBalancerHandler;

/**
 * This exception is fired when {@link LoadBalancerHandler} receives
 * invalid Load Balancer start request.
 */
public final class InvalidLoadBalancerStartRequestException extends Exception {

    public InvalidLoadBalancerStartRequestException() {
    }

    public InvalidLoadBalancerStartRequestException(String message) {
        super(message);
    }

    public InvalidLoadBalancerStartRequestException(String message, Throwable cause) {
        super(message, cause);
    }

    public InvalidLoadBalancerStartRequestException(Throwable cause) {
        super(cause);
    }

    public InvalidLoadBalancerStartRequestException(String message, Throwable cause, boolean enableSuppression, boolean writableStackTrace) {
        super(message, cause, enableSuppression, writableStackTrace);
    }
}
