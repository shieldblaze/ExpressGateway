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
/*
 * The MIT License
 *
 * Copyright (c) 2008-2011, Sun Microsystems, Inc., Kohsuke Kawaguchi,
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 */
package com.shieldblaze.expressgateway.metrics;

import com.shieldblaze.expressgateway.common.Math;

import java.io.BufferedReader;
import java.io.FileReader;
import java.io.IOException;
import java.util.Arrays;

/**
 * System Memory Load
 */
public final class Memory {

    public MemoryUsage memory() throws IOException {
        try (BufferedReader r = new BufferedReader(new FileReader("/proc/meminfo"))) {
            long[] values = new long[4];
            Arrays.fill(values, -1);

            String line;
            while ((line = r.readLine()) != null) {
                for (int i = 0; i < HEADERS.length; i++) {
                    if (line.startsWith(HEADERS[i])) {
                        // found a line that we care about
                        String s = line.substring(HEADERS[i].length()).trim();

                        // trim off the suffix, if any
                        Suffix suffix = Suffix.find(s);
                        s = s.substring(0, s.length() - suffix.name.length()).trim();

                        try {
                            values[i] = Long.parseLong(s) * suffix.multiplier;
                        } catch (NumberFormatException e) {
                            throw new IOException("Failed to parse: '" + s + "' out of '" + line + "'");
                        }
                        break;
                    }
                }
            }

            return new MemoryUsage(values);
        }
    }

    /**
     * Lines in <tt>/proc/meminfo</tt> that we care about.
     */
    private static final String[] HEADERS = new String[]{
            "MemTotal:",
            "MemFree:",
            "SwapTotal:",
            "SwapFree:"
    };

    /**
     * I've only seen "1234 kB" notation, but just to be safe.
     */
    private static final Suffix[] SUFFIXES = new Suffix[]{
            new Suffix("KB", 1024),
            new Suffix("MB", 1024 * 1024),
            new Suffix("GB", 1024 * 1024 * 1024)
    };

    private static final Suffix NONE = new Suffix("", 1);

    private static class Suffix {
        final String name;
        final long multiplier;

        private Suffix(String name, long multiplier) {
            this.name = name;
            this.multiplier = multiplier;
        }

        static Suffix find(String line) {
            for (Suffix s : SUFFIXES)
                if (line.substring(line.length() - s.name.length()).equalsIgnoreCase(s.name))
                    return s;
            return NONE;
        }
    }

    public static final class MemoryUsage {
        /**
         * Total physical memory of the system, in bytes.
         * -1 if unknown.
         */
        private final long totalPhysicalMemory;

        /**
         * Of the total physical memory of the system, available bytes.
         * -1 if unknown.
         */
        private final long availablePhysicalMemory;

        /**
         * Total number of swap space in bytes.
         * -1 if unknown.
         */
        private final long totalSwapSpace;

        /**
         * Available swap space in bytes.
         * -1 if unknown.
         */
        private final long availableSwapSpace;

        public MemoryUsage(long totalPhysicalMemory, long availablePhysicalMemory, long totalSwapSpace, long availableSwapSpace) {
            this.totalPhysicalMemory = totalPhysicalMemory;
            this.availablePhysicalMemory = availablePhysicalMemory;
            this.totalSwapSpace = totalSwapSpace;
            this.availableSwapSpace = availableSwapSpace;
        }

        MemoryUsage(long[] v) throws IOException {
            this(v[0], v[1], v[2], v[3]);
            if (!hasData(v))
                throw new IOException("No data available");
        }

        public long totalPhysicalMemory() {
            return totalPhysicalMemory;
        }

        public long availablePhysicalMemory() {
            return availablePhysicalMemory;
        }

        public long totalSwapSpace() {
            return totalSwapSpace;
        }

        public long availableSwapSpace() {
            return availableSwapSpace;
        }

        public float physicalMemoryUsed() {
            return Math.percentage(availablePhysicalMemory, totalPhysicalMemory);
        }

        public float swapSpaceUsed() {
            return Math.percentage(availableSwapSpace, totalSwapSpace);
        }

        private static long toMB(long l) {
            return l / (1024 * 1024);
        }

        /*package*/
        static boolean hasData(long[] values) {
            for (long v : values)
                if (v != -1) return true;
            return false;
        }
    }
}
