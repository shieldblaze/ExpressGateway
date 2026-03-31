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
package com.shieldblaze.expressgateway.protocol.http;

/**
 * Shared URI normalization utilities used by HTTP/1.1, HTTP/2, and HTTP/3 inbound handlers.
 *
 * <p>Implements RFC 3986 Section 5.2.4 dot-segment removal with additional security checks:</p>
 * <ul>
 *   <li>Null byte rejection (SEC-06)</li>
 *   <li>Double-encoded dot detection (SEC-05)</li>
 *   <li>Path traversal detection (escapesRoot)</li>
 * </ul>
 *
 * <p>Extracted from {@link Http11ServerInboundHandler} to enable reuse by
 * {@code Http3ServerHandler} (DEF-H3-URI fix).</p>
 */
public final class UriNormalizer {

    private UriNormalizer() {
    }

    /**
     * Normalizes a request URI by removing dot segments per RFC 3986 Section 5.2.4,
     * with additional security checks for null bytes, double-encoded dots, and
     * path traversal.
     *
     * @param uri the raw request URI (e.g., "/a/b/../c?q=1")
     * @return the normalized URI, or {@code null} if the path attempts to escape
     *         the document root or contains prohibited patterns
     */
    public static String normalizeUri(String uri) {
        if (uri == null || uri.isEmpty()) {
            return uri;
        }

        // Asterisk-form ("OPTIONS * HTTP/1.1") — nothing to normalize.
        if ("*".equals(uri)) {
            return uri;
        }

        // Split off the query string — normalization applies only to the path.
        String path;
        String query;
        int qIdx = uri.indexOf('?');
        if (qIdx >= 0) {
            path = uri.substring(0, qIdx);
            query = uri.substring(qIdx); // includes the '?'
        } else {
            path = uri;
            query = "";
        }

        // If the path is empty (e.g., "?foo=bar"), leave it alone.
        if (path.isEmpty()) {
            return uri;
        }

        // SEC-06: Reject null bytes in path.
        if (path.indexOf('\0') >= 0 || containsIgnoreCase(path, "%00")) {
            return null;
        }

        // SEC-05: Reject double-encoded dots in path.
        if (containsDoubleEncodedDot(path)) {
            return null;
        }

        // Decode percent-encoded dots (%2e/%2E) before path traversal check.
        String decodedPath = path;
        if (decodedPath.contains("%")) {
            decodedPath = decodedPath.replace("%2e", ".").replace("%2E", ".");
        }

        // Traversal depth check: if ".." ever causes depth below zero, reject.
        if (decodedPath.startsWith("/") && escapesRoot(decodedPath)) {
            return null;
        }

        // Fast path — no dot segments to remove.
        if (decodedPath.indexOf('.') < 0) {
            return uri;
        }

        String normalized = removeDotSegments(path);
        return normalized + query;
    }

    /**
     * Checks whether the given absolute path would escape the document root
     * by counting segment depth.
     */
    public static boolean escapesRoot(String path) {
        int depth = 0;
        int i = 0;
        int len = path.length();

        while (i < len) {
            while (i < len && path.charAt(i) == '/') {
                i++;
            }
            int segStart = i;
            while (i < len && path.charAt(i) != '/') {
                i++;
            }
            int segLen = i - segStart;

            if (segLen == 0) {
                continue;
            } else if (segLen == 1 && path.charAt(segStart) == '.') {
                continue;
            } else if (segLen == 2 && path.charAt(segStart) == '.' && path.charAt(segStart + 1) == '.') {
                depth--;
                if (depth < 0) {
                    return true;
                }
            } else {
                depth++;
            }
        }
        return false;
    }

    /**
     * SEC-05: Checks whether the path contains a double-encoded dot sequence (%252e).
     */
    public static boolean containsDoubleEncodedDot(String path) {
        int len = path.length();
        for (int i = 0; i <= len - 5; i++) {
            if (path.charAt(i) == '%'
                    && path.charAt(i + 1) == '2'
                    && path.charAt(i + 2) == '5'
                    && path.charAt(i + 3) == '2'
                    && (path.charAt(i + 4) == 'e' || path.charAt(i + 4) == 'E')) {
                return true;
            }
        }
        return false;
    }

    /**
     * Case-insensitive substring containment check without allocation.
     */
    static boolean containsIgnoreCase(String haystack, String needle) {
        int hLen = haystack.length();
        int nLen = needle.length();
        if (nLen > hLen) {
            return false;
        }
        outer:
        for (int i = 0; i <= hLen - nLen; i++) {
            for (int j = 0; j < nLen; j++) {
                char h = Character.toLowerCase(haystack.charAt(i + j));
                char n = needle.charAt(j);
                if (h != n) {
                    continue outer;
                }
            }
            return true;
        }
        return false;
    }

    /**
     * Implements the remove_dot_segments algorithm from RFC 3986 Section 5.2.4.
     */
    public static String removeDotSegments(String path) {
        StringBuilder input = new StringBuilder(path);
        StringBuilder output = new StringBuilder(path.length());

        while (!input.isEmpty()) {
            if (startsWith(input, "../")) {
                input.delete(0, 3);
            } else if (startsWith(input, "./")) {
                input.delete(0, 2);
            } else if (startsWith(input, "/./")) {
                input.delete(0, 2);
            } else if (equals(input, "/.")) {
                input.replace(0, input.length(), "/");
            } else if (startsWith(input, "/../")) {
                input.delete(0, 3);
                removeLastSegment(output);
            } else if (equals(input, "/..")) {
                input.replace(0, input.length(), "/");
                removeLastSegment(output);
            } else if (equals(input, ".") || equals(input, "..")) {
                input.setLength(0);
            } else {
                int segStart = 0;
                if (input.charAt(0) == '/') {
                    segStart = 1;
                }
                int nextSlash = indexOf(input, '/', segStart);
                if (nextSlash < 0) {
                    output.append(input);
                    input.setLength(0);
                } else {
                    output.append(input, 0, nextSlash);
                    input.delete(0, nextSlash);
                }
            }
        }

        return output.toString();
    }

    private static boolean startsWith(StringBuilder sb, String prefix) {
        if (sb.length() < prefix.length()) return false;
        for (int i = 0; i < prefix.length(); i++) {
            if (sb.charAt(i) != prefix.charAt(i)) return false;
        }
        return true;
    }

    private static boolean equals(StringBuilder sb, String str) {
        if (sb.length() != str.length()) return false;
        return startsWith(sb, str);
    }

    private static int indexOf(StringBuilder sb, char ch, int fromIndex) {
        for (int i = fromIndex; i < sb.length(); i++) {
            if (sb.charAt(i) == ch) return i;
        }
        return -1;
    }

    private static void removeLastSegment(StringBuilder output) {
        int lastSlash = output.lastIndexOf("/");
        if (lastSlash >= 0) {
            output.setLength(lastSlash);
        } else {
            output.setLength(0);
        }
    }
}
