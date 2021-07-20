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
package com.shieldblaze.expressgateway.integration.server;

import com.shieldblaze.expressgateway.integration.event.FleetScaleInEvent;
import com.shieldblaze.expressgateway.integration.event.FleetScaleOutEvent;

import java.util.List;

/**
 * This class is used for scaling in or out.
 *
 * @param <IN>  Scale In type
 * @param <OUT> Scale out type
 */
public interface FleetManager<IN, OUT> {

    /**
     * List of {@link Server} in the Fleet
     */
    List<Server> servers();

    /**
     * Scale In server in the Fleet
     */
    FleetScaleInEvent<?> scaleIn(IN obj);

    /**
     * Scale Out server in the Fleet
     */
    FleetScaleOutEvent<?> scaleOut(OUT obj);
}
