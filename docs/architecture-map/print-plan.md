# Wall-map preparation plan

The master SVG canvas is 3000 × 1680 (aspect ratio about 1.79:1), so it is
well-suited to a landscape technical map. For its current node density, print
it as a **4 columns × 3 rows** grid of US Letter sheets in landscape orientation
(11 × 8.5 in each). That produces an assembled map of roughly 42.6 × 24.6 in
after allowing 0.25 in overlap on internal joins; the final aspect ratio remains
close to the SVG and leaves good readability at a wall distance.

Use 0.25 in overlap on every interior edge. Borderless printing is preferred;
without it, print with a 0.25–0.35 in unprinted margin and use the overlap as a
trim zone. Add crop marks outside artboard content and centered alignment ticks
on every shared edge. Label pages `R1C1` through `R3C4` in the outer margin and
repeat the page label at the opposite edge so a trimmed stack remains sortable.

At the recommended scale the source’s 20–23 px node detail labels render near
8–9 pt and main titles near 12–14 pt. Do not reduce below 8 pt for the smallest
path/type text. Before physical tiling, render the SVG to a vector PDF at the
full 44 × 25 in artboard, inspect one center page and one densely connected
middle seam, then add registration/crop marks in the tiling step. Keep the DOT
and SVG together as the canonical editable master sources.
