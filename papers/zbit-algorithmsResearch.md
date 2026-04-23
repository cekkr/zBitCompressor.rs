<!-- Licensed under the PolyForm Noncommercial License 1.0.0. See [LICENSE](../LICENSE). -->
<!-- Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>. -->

# A Comprehensive Analysis of

# Algorithmic Approaches for Two-Level

# Logic Minimization

## Section 1: The Landscape of Logic Minimization

The design of efficient digital circuits is a cornerstone of modern computing and electronics.
At its heart lies the problem of logic minimization: the process of transforming a Boolean
function, which describes a circuit's behavior, into its simplest equivalent form.^1 A simplified
function translates directly into a more optimized physical circuit with tangible benefits,
including reduced complexity and component count (fewer logic gates), lower manufacturing
cost, improved operational speed due to shorter signal propagation delays, and decreased
power consumption.^1 This report provides a comprehensive, expert-level analysis of the
principal algorithms used to achieve this optimization, intended to guide the implementation
of a performant and scalable logic minimizer.

### 1.1. Beyond the Karnaugh Map: The Imperative for Algorithmic

### Solutions

For students and engineers first encountering digital logic, the Karnaugh Map (K-map) is the
primary tool for Boolean simplification.^2 A K-map is a graphical method that arranges the
minterms of a truth table into a grid. The key feature of this grid is that its cells are ordered
using Gray code, a binary numeral system where two successive values differ in only one bit.^6
This arrangement ensures that logically adjacent minterms—those that can be combined to
eliminate a variable—are also physically adjacent in the map. The process of minimization
involves visually identifying and grouping clusters of '1's (for Sum-of-Products, SOP, form) or
'0's (for Product-of-Sums, POS, form).^8 These groups must contain a number of cells that is a
power of two (1, 2, 4, 8, etc.), and the larger the group, the more variables are eliminated,
leading to a simpler product term.^6
While K-maps are an excellent pedagogical tool for functions with a small number of variables
(typically two to four, and up to five with significant effort), they suffer from a severe and
fundamental limitation: a complete lack of scalability.^4 The number of cells in a K-map grows
exponentially with the number of input variables,


n, as 2n.^6 A 4-variable function requires a 16-cell map, a 5-variable function requires a 32-cell
map (often visualized as two 4x4 grids), and a 6-variable function requires a 64-cell map,
which must be represented as a three-dimensional structure of four 4x4 grids.^9 Beyond this
point, the visual pattern-recognition process that makes K-maps intuitive breaks down
completely. The adjacencies become impossible for a human to track, and the method
becomes exceptionally error-prone and impractical.^6
Modern digital systems, from microprocessors to complex application-specific integrated
circuits (ASICs), involve logic functions with tens or even hundreds of variables. For these
real-world applications, manual methods are not just inefficient; they are impossible. This
"curse of dimensionality" creates an imperative for systematic, computer-driven algorithms
that can navigate this vast combinatorial space to find minimal or near-minimal circuit
representations.^11

### 1.2. Defining "Best": The Trilemma of Optimality, Performance, and

### Scalability

When seeking the "best" algorithm for logic minimization, it is crucial to understand that
"best" is not a monolithic concept. The selection of an algorithm involves navigating a
fundamental trade-off, a trilemma between three competing objectives: optimality,
performance, and scalability. The choice of which objective to prioritize depends entirely on
the specific application and its constraints.
● **Optimality:** This refers to the quality of the final solution. An algorithm is considered
"exact" or "optimal" if it guarantees finding a mathematically minimal representation of
the Boolean function.^13 However, the definition of "minimal" itself can be nuanced. The
primary goal is often to find a Sum-of-Products (SOP) expression with the fewest
possible product terms, as this directly corresponds to the number of AND gates
feeding a final OR gate in a Programmable Logic Array (PLA) implementation. A
secondary goal, particularly for multi-level logic synthesis, is to minimize the total
number of literals (variables, complemented or not) across all terms. A solution with the
fewest terms is not always the one with the fewest literals.^11 An exact algorithm aims to
satisfy these criteria perfectly, typically by finding all solutions with the minimum
number of terms and, from that set, selecting one with the minimum number of literals.^11
● **Performance (Speed):** This measures the computational time required for an algorithm
to produce a result. It is directly related to the algorithm's time complexity. An algorithm
that is theoretically optimal may take an exponential amount of time to run, rendering it
useless for all but the smallest problems.^14 In many design contexts, a fast algorithm
that produces a "good enough" solution is far more valuable than a slow one that
produces a perfect solution.
● **Scalability:** This is a measure of how an algorithm's performance, in terms of both time
and memory usage, degrades as the size of the problem increases.^6 Problem size is a


function of the number of input variables, output functions, and the number of
minterms in the ON-set. An algorithm is considered scalable if it can handle large,
industrial-scale problems with many variables and complex functions without an
exponential explosion in resource requirements.
The challenge for the implementer of a logic minimizer is that these three goals are often
mutually exclusive. An algorithm can be optimal, but it will likely be slow and not scale well. An
algorithm can be fast and scalable, but it may have to sacrifice the guarantee of finding the
absolute best solution. This report will analyze each algorithmic approach through the lens of
this trilemma, providing the necessary context to make an informed implementation decision.

## Section 2: Exact Algorithms: The Quest for the True

## Minimum

For applications where guaranteed optimality is paramount, such as in academic research, the
verification of other heuristics, or the design of critical, highly constrained circuits, exact
algorithms are the required tool. These methods systematically explore the entire solution
space to find a provably minimal logic representation. The canonical algorithm in this class is
the Quine-McCluskey method.

### 2.1. The Quine-McCluskey (Q-M) Method

The Quine-McCluskey (Q-M) method, also known as the tabular method, is a systematic
procedure for minimizing Boolean functions that is functionally equivalent to the K-map but is
not limited by human visualization capabilities.^1 It provides a deterministic, programmable path
to finding an exact minimal sum-of-products expression.^11 The method's power and its
weakness both stem from its clean, two-phase approach: first, it exhaustively generates all
possible candidate terms (prime implicants), and second, it solves the problem of selecting
the minimal set of those terms needed to cover the function.

#### 2.1.1. Core Principle and Mathematical Foundation

The mathematical foundation of the Q-M method is the repeated application of the Boolean
adjacency or combining theorem: XY+XY′=X.^16 This theorem states that if two product terms
are identical except for one variable that appears in its true form in one term and its
complemented form in the other, the two terms can be combined into a single, simpler term
with that variable eliminated. The Q-M algorithm is essentially a structured, exhaustive
application of this rule to ensure that every possible simplification is discovered. The terms
generated by this process that cannot be simplified any further are known as


**prime implicants**. A prime implicant is an implicant (a product term that does not cover any
minterms in the function's OFF-set) that would cease to be an implicant if any literal were
removed from it.^17 Finding the complete set of prime implicants is a necessary prerequisite for
finding a minimal cover, as any minimal SOP solution must be composed entirely of prime
implicants.

#### 2.1.2. Algorithmic Step 1: Generation of Prime Implicants

The first phase of the Q-M algorithm is a tabular method for generating the complete set of
prime implicants for a given function.
● **Input:** The algorithm begins with a list of the minterms (and any don't-care terms) for
which the function's output is '1'.^11 These are typically specified by their decimal
equivalents.
● **Grouping by Index:** Each minterm is converted to its binary representation. The
minterms are then sorted into groups based on their **index** —the number of '1's in their
binary string.^16 For example, for a 4-variable function, minterm 1 (0001) would be in
Group 1, minterm 3 (0011) in Group 2, and minterm 7 (0111) in Group 3. This grouping is a
crucial optimization; it drastically reduces the number of comparisons required, as a
term from Group
k can only combine with a term from Group k+1.^17
● **Iterative Combination (Column I to Column II):** The algorithm proceeds by comparing
every term in Group k with every term in Group k+1. If a pair of terms differs by exactly
one bit position, they are combined. A new term is formed in a second column, which is
identical to the parent terms but with a dash (-) placed in the bit position where they
differed. This dash signifies that the corresponding variable has been eliminated via the
combining theorem.^16 For example, minterms 1 (0001) and 5 (0101) would combine to
form the term
0-01. Both parent terms (1 and 5) are then marked with a check (✓) to indicate they
have been included in a larger implicant.^16
● **Higher-Order Combination (Subsequent Columns):** The process is repeated with the
terms in the second column. New groups are formed based on the index of the new
terms. Two terms from adjacent groups in the second column can be combined only if
they satisfy two conditions: (1) their dashes are in the same position, and (2) they differ
by exactly one other bit position.^16 This process continues, creating subsequent
columns of larger and larger implicants, until no further combinations can be made.
● **Termination and PI Identification:** The process terminates when a full pass through
the latest column yields no new combinations. The complete set of prime implicants
consists of all the terms generated throughout the process that were never checked
off.^15


#### 2.1.3. Algorithmic Step 2: The Prime Implicant Chart

Once all prime implicants (PIs) have been generated, the second phase begins: selecting a
minimal subset of these PIs that covers all the original ON-set minterms. This is a classic
set-covering problem.
● **Construction:** A two-dimensional table, the prime implicant chart, is constructed. The
rows correspond to the prime implicants, and the columns correspond to the original
ON-set minterms (don't-care terms are not listed as columns because they do not need
to be covered).^16 An 'X' is placed at the intersection of a row and column if the prime
implicant in that row covers the minterm in that column.^19
● **Identifying Essential Prime Implicants (EPIs):** The first step in solving the chart is to
identify any **essential prime implicants**. An EPI is a PI that provides the sole coverage
for at least one minterm.^16 In the chart, this is easily identified as any column that
contains only a single 'X'. The PI corresponding to that row is essential and must be
included in the final minimal solution. Once an EPI is selected, its row and all the
columns (minterms) it covers are removed from the chart, simplifying the remaining
problem.^19
● **Simplifying with Row and Column Dominance:** After all EPIs are handled, the chart
can often be simplified further using dominance rules 19 :
○ **Row Dominance:** If row PI_A has an 'X' in every column that row PI_B has an 'X'
(and possibly more), then PI_A is said to dominate PI_B. Since PI_A can cover
everything PI_B can (and they have the same cost: one product term), the
dominated row PI_B can be removed from the chart.
○ **Column Dominance:** If column m_C has an 'X' in every row that column m_D has
an 'X' (and possibly more), then m_C is said to dominate m_D. This means that any
PI that covers m_D will automatically cover m_C. Therefore, we only need to worry
about covering m_D, and the dominating column m_C can be removed.

#### 2.1.4. Solving the Reduced Chart: Petrick's Method

If the chart is reduced as much as possible via EPI selection and dominance but is still not
empty, it is called a **cyclic core**. This requires a more complex procedure to find the minimal
cover. Petrick's method is an algebraic technique for finding all minimal solutions from a cyclic
chart.^13

1. A Boolean expression P is constructed. For each remaining minterm column m_ j, a sum
    term S_ j is created, where S_ j = (PI_1 + PI_2 +...) lists the PIs that cover m_ j.
2. These sum terms are all ANDed together: P=S1 ⋅S2 ⋅⋯⋅Sk.
3. This expression P is then multiplied out into a canonical sum-of-products form using
    Boolean laws, primarily the distributive law A(B+C)=AB+AC and idempotency A+A=A.
4. Each product term in the resulting SOP for P represents a valid set of PIs that covers all

## A stronger implementation-oriented view of Boolean optimization

```
the minterms. For example, a term like PI1 PI3 PI5 represents a cover consisting of those
three prime implicants.
```
5. To find the minimal solution, we first identify the product terms with the fewest literals
    (i.e., the fewest PIs). If there is a tie, we then calculate the total literal cost for each of
    those solutions and select the one(s) with the true minimum cost.

#### 2.1.5. Implementation and Complexity Analysis

The Q-M method is entirely systematic and can be readily programmed on a computer.^11
However, its practical utility is severely limited by its computational complexity. The algorithm
faces two potential exponential bottlenecks. First, in the worst case, the number of prime
implicants can grow exponentially with the number of variables
n (on the order of 3n/n). Second, the covering problem solved by the prime implicant chart is
equivalent to the set cover problem, which is NP-hard. Petrick's method, in particular, can
generate an intermediate expression with a doubly exponential number of terms. This "double
jeopardy" of complexity means that Q-M becomes computationally intractable for functions
with more than approximately 15-20 variables, making it unsuitable for most modern logic
synthesis tasks.^13 Its value lies in its guaranteed optimality, which serves as a benchmark
against which faster, heuristic methods are measured.

## Section 3: Heuristic Algorithms: The Pragmatic

## Industry Standard

The exponential complexity of exact algorithms like Quine-McCluskey necessitated the
development of heuristic methods capable of handling the large-scale problems found in
industrial circuit design. Heuristic algorithms trade the guarantee of absolute optimality for
dramatic improvements in performance and scalability. They employ sophisticated "rules of
thumb" to navigate the vast solution space and find a high-quality, near-minimal solution in a
practical amount of time. The most famous and influential algorithm in this category is the
Espresso logic minimizer.

### 3.1. The Espresso Heuristic

Developed at IBM and refined at the University of California, Berkeley, Espresso represents a
paradigm shift from the exhaustive search of Q-M to an iterative, improvement-based
strategy.^21 It is often described as an "anytime" algorithm because it maintains a complete,

valid cover for the function at all stages of its execution; it simply gets better over time.^12 This
robustness and its ability to efficiently handle functions with many inputs and outputs have


made it the de facto industry standard for two-level logic minimization for decades.^12

#### 3.1.1. Philosophy and Rationale

Espresso's core philosophy is to avoid the two computational cliffs of the Q-M method: it does
not attempt to generate all prime implicants, nor does it attempt to solve the covering problem
exactly. Instead, it starts with an initial cover of the function and iteratively refines it using a
sequence of operators, each designed to improve the cover's quality (typically measured by
the number of product terms and literals).^14 The process continues until no further
improvements can be made, at which point the algorithm has converged to a local
minimum—one that is, in practice, very often the global minimum or extremely close to it.^12

#### 3.1.2. Representing the Function: The Cover and Cubes

Espresso operates on a specific representation of the Boolean function. The function is
defined by three distinct sets of **cubes** (product terms) 14 :
● **ON-set (F):** The set of cubes that must be covered. This represents the minterms for
which the function's output is 1.
● **OFF-set (R):** The set of cubes that must _not_ be covered. This represents the minterms
for which the function's output is 0.
● **DC-set (D):** The set of don't-care cubes. These can be used to help simplify the
ON-set cover but do not themselves need to be covered.
The goal of the algorithm is to find a new cover, C, for the function such that it covers the
entire ON-set (F⊆C∪D) without intersecting the OFF-set (C∩R=∅), while minimizing a cost
function, typically the number of cubes in C. Input and output are commonly handled using
the PLA (Programmable Logic Array) format, which explicitly lists the input conditions and
corresponding output activations for each product term.^24

#### 3.1.3. The Core Iterative Loop

The power of Espresso lies in its core loop, which repeatedly applies a sequence of
specialized operators to the current cover. The canonical loop is EXPAND → IRREDUNDANT →
REDUCE, which is repeated until the cost of the cover no longer decreases.^22

**A. The EXPAND Operator**

```
● Goal: To reduce the number of product terms by making each individual term as large
as possible. A larger (or less-defined) cube contains fewer literals and covers a larger
```

```
area of the Boolean space. By expanding a cube, it may cover minterms that were
previously covered by other cubes, potentially making those other cubes redundant and
removable in a later step.
● Process: For each cube c in the current cover, the EXPAND operator attempts to grow it
into a prime implicant. It does this by removing literals from the cube one by one. A
literal can be removed if the resulting, larger cube does not intersect with the OFF-set.
The DC-set can be used to facilitate this expansion, as the cube is allowed to grow into
the don't-care space. The order in which cubes are expanded and the direction of their
expansion (which literals to try removing first) are guided by clever heuristics. The goal
is to find "good" prime implicants—those that are most likely to cover ON-set minterms
that are not yet covered by other expanded primes.^14
```
**B. The IRREDUNDANT Operator**

```
● Goal: To remove as many redundant cubes as possible from the cover, directly reducing
the primary cost metric (the term count).
● Process: After EXPAND, the cover is likely to contain significant redundancy, as many
cubes will have grown to cover overlapping regions. The IRREDUNDANT operator seeks
a minimal subset of the current cover that is still a valid cover. It first identifies all
relatively essential cubes—those that cover at least one ON-set minterm not covered
by any other cube in the current set. These are marked for keeping. For the remaining
cubes, the problem is a classic unate covering problem. IRREDUNDANT uses a heuristic
algorithm to select a minimal number of the remaining cubes to cover all the remaining
ON-set minterms.^14
```
**C. The REDUCE Operator**

```
● Goal: To shrink each cube in the cover to be as small as possible while ensuring the
collection of cubes as a whole still covers the original function. This step appears
counter-intuitive, as the goal of minimization is generally to create larger cubes.
However, REDUCE is the key to the algorithm's ability to escape local minima.
● Process: For each cube c in the cover, REDUCE determines the smallest sub-cube of c
that is sufficient to cover the essential parts of the ON-set that c is responsible for (i.e.,
the minterms covered by c but not by any other cube in the cover). By shrinking the
cubes, REDUCE creates "space" in the Boolean map. This perturbation of the current
solution is critical. In the next iteration, the EXPAND operator will have a different
landscape to work with. The newly created space may allow it to expand cubes in
different directions, discovering new, potentially more effective prime implicants that
were previously blocked by the larger cubes of the previous iteration.^14 This symbiotic
tension between
EXPAND's greedy expansion and REDUCE's strategic contraction is what allows
```

```
Espresso to effectively explore the solution space and settle on a high-quality local
minimum.
```
#### 3.1.4. Implementation Details

Espresso is designed to be a practical tool. It can be configured with numerous options to
control the minimization process, such as performing an exact minimization for smaller
sub-problems or optimizing for multiple-output functions simultaneously.^24 By sharing
product terms among several output functions, a multi-output minimizer can achieve a far
more compact global implementation than by minimizing each output function independently.
The algorithm's iterative nature and its focus on manipulating sets of cubes make it
well-suited for efficient implementation in software.

## Section 4: Graph-Based Minimization with Binary

## Decision Diagrams

A fundamentally different approach to logic representation and minimization moves away from
manipulating algebraic expressions (like SOP forms) and instead utilizes a graphical
representation of Boolean functions. This paradigm is centered on the **Binary Decision
Diagram (BDD)** , a data structure that can provide a canonical and often highly compact
representation of logic, making it a cornerstone of modern formal verification and logic
synthesis.

### 4.1. From Expressions to Graphs: The Binary Decision Diagram (BDD)

A Binary Decision Diagram is a directed acyclic graph (DAG) used to represent a Boolean
function.^29 The structure is derived directly from the
Shannon expansion theorem, which states that any Boolean function f can be decomposed
with respect to a variable v as:
f=(v∧fv )∨(¬v∧f¬v )
Here, fv and f¬v are the cofactors of f, obtained by setting v to 1 and 0, respectively.
A BDD graph consists of:
● **Decision Nodes:** Each non-terminal node is labeled with an input variable.
● **Edges:** Each decision node has two outgoing edges: a **high edge** (or then edge), taken
when the node's variable is 1, and a **low edge** (or else edge), taken when the variable is
0.
● **Terminal Nodes:** The graph has two terminal nodes, representing the constant Boolean
values 0 (FALSE) and 1 (TRUE).^30


To evaluate the function for a given set of input values, one traverses a path from the single
**root node** down to a terminal node, following the high or low edge at each decision node
based on the value of that node's variable. The terminal node reached at the end of the path
gives the value of the function for that input combination.^30

### 4.2. The Power of Canonicity: Reduced Ordered BDDs (ROBDDs)

An unconstrained BDD offers little advantage and can be as large and redundant as a full truth
table or decision tree.^33 The power of BDDs is unlocked by applying two critical constraints to
create a
**Reduced Ordered Binary Decision Diagram (ROBDD)**.^30

1. **Ordering:** A fixed variable ordering is imposed. Along any path from the root to a
    terminal, variables must be encountered in this specific, predefined order. A variable
    can appear at most once on any path. This creates an **Ordered BDD (OBDD)**.^29
2. **Reduction:** Two simple but powerful reduction rules are applied recursively from the
    terminal nodes up to the root 30 :
       ○ **Merge Isomorphic Subgraphs:** Identify any two nodes in the graph that are
          isomorphic—that is, they are labeled with the same variable and their high and
          low edges point to the same respective child nodes. These nodes represent the
          exact same sub-function. All incoming edges to one of the nodes are redirected
          to the other, and the redundant node is eliminated.
       ○ **Eliminate Redundant Nodes:** Identify any decision node whose high and low
          edges both point to the same child node. This node is redundant because the
          decision on its variable has no effect on the outcome. The node is removed, and
          all its incoming edges are redirected to its child.
The most important property of an ROBDD is its **canonicity** : for a given Boolean function and
a fixed variable ordering, the ROBDD is unique.^30 This means that any two functions are
logically equivalent if and only if their ROBDDs, built with the same variable order, are
identical. This property makes ROBDDs an exceptionally powerful tool for formal equivalence
checking, a critical task in circuit verification.^29

### 4.3. Logic Synthesis and Minimization with ROBDDs

Logic minimization using ROBDDs is not an explicit procedure like Q-M or Espresso. Instead, it
is an emergent property of the data structure and its manipulation. The reduction rules
themselves perform a type of logic simplification by eliminating redundancy.^34 The primary
lever for minimization is
**variable ordering**.
The size of an ROBDD (the number of nodes) for a given function is highly sensitive to the
chosen variable ordering. For the same function, one ordering can result in an ROBDD with a


number of nodes that is linear in the number of variables, while another ordering can lead to
an exponential explosion in size.^29 A classic example is the function for a ripple-carry adder; a
good variable ordering (e.g., interleaving the bits of the two inputs) yields a linear-sized
ROBDD, whereas a poor ordering (e.g., all bits of the first input followed by all bits of the
second) results in an exponential-sized ROBDD.^30

Finding the absolute optimal variable ordering for a function is, itself, an NP-hard problem.^35
Therefore, practical logic synthesis systems that use BDDs employ sophisticated heuristics to
find a good, near-optimal ordering. These include
**dynamic variable reordering** techniques, such as the "sifting" algorithm, which iteratively
moves each variable up and down through the order to find its locally optimal position.^35 By
minimizing the size of the ROBDD graph through intelligent ordering, one is implicitly finding a
more compact and efficient representation of the logic function.
This approach fundamentally reframes the minimization problem. Instead of manipulating
terms in an expression, the problem becomes one of graph optimization. This abstraction is
powerful; when a function has a structure that admits a compact ROBDD representation, this
data structure can implicitly represent and manipulate an exponential number of product
terms using a polynomial-sized graph. Furthermore, because operations like AND, OR, and
XOR can be performed directly on ROBDDs (typically via a universal ITE or If-Then-Else
operator), an entire synthesis flow can be built around this canonical data structure, getting
powerful capabilities like equivalence checking "for free".^31 This provides a significant
system-level advantage that is absent in purely expression-based minimizers.

## Section 5: Modern Approaches: Leveraging

## General-Purpose Solvers

A powerful trend in algorithm design is to recast specialized problems into a format that can
be solved by highly optimized, general-purpose engines. In logic minimization, this has led to
the development of methods that leverage the remarkable power of modern **Boolean
Satisfiability (SAT) solvers**. This approach translates the sub-problems of logic minimization
into satisfiability queries, offloading the complex search process to a tool that has been the
subject of decades of intensive research and optimization.

### 5.1. Logic Minimization as a Satisfiability Problem

#### 5.1.1. Introduction to SAT Solvers

The Boolean Satisfiability problem (SAT) is the classic problem of determining if there exists


an assignment of truth values (TRUE/FALSE) to the variables of a given Boolean formula that
makes the entire formula evaluate to TRUE.^36 A program that solves this problem is called a
SAT solver.
While SAT is the original NP-complete problem, meaning no known algorithm can solve it in
polynomial time in the worst case, the practical performance of modern SAT solvers is
extraordinary.^14 These solvers, typically based on the Davis-Putnam-Logemann-Loveland
(DPLL) algorithm enhanced with Conflict-Driven Clause Learning (CDCL), can often solve
industrial-scale problems with hundreds of thousands or even millions of variables and
constraints.^14
The standard input for most SAT solvers is a formula in **Conjunctive Normal Form (CNF)**. A
CNF formula is a logical AND (conjunction) of one or more **clauses** , where each clause is a
logical OR (disjunction) of one or more **literals** (a variable or its negation).^38 For example,
(x1 ∨¬x2 )∧(x2 ∨x3 ) is a CNF formula. If a formula is satisfiable, the solver returns a satisfying
assignment (a model); if not, it returns UNSAT.^36

### 5.2. SAT-ESPRESSO: A SAT-Based Implementation of Heuristic

### Minimization

This approach does not seek to invent a new high-level minimization strategy from scratch.
Instead, it takes the proven, high-quality heuristic framework of the Espresso algorithm and
re-implements its core, computationally intensive operators by translating them into SAT
problems.^14 This hybrid method, exemplified by the SAT-ESPRESSO minimizer, aims to
combine the sophisticated logic minimization strategies of Espresso with the raw search
performance of modern SAT solvers, achieving significant speedups on large problems.^14 The
focus is on replacing the procedural logic of Espresso's bottleneck operators—
REDUCE, IRREDUNDANT, and ESSENTIALS—with declarative SAT formulations.

#### 5.2.1. SAT-based REDUCE

```
● Goal: For a given cube c in a cover C, find its maximal reduction—the smallest possible
sub-cube of c that is still sufficient to cover the ON-set minterms that c is uniquely
responsible for.
● Formulation: The core task is to identify the set of minterms that must be covered by
the reduced version of c. A minterm m has this property if it satisfies three conditions
simultaneously:
```
1. m is in the ON-set of the function.
2. m is contained within the original cube c.
3. m is _not_ contained in any other cube in the cover C - {c}.
● **Process:** These three conditions can be encoded into a single Boolean formula whose
variables are the input variables of the function. This formula is then converted to CNF

---

The original document correctly explains classic **two-level logic minimization**, but it is too narrow for serious circuit compression. In modern synthesis, the best results rarely come from choosing a single minimizer. They come from choosing the **right representation**, applying the **right local and global transformations**, exploiting **don't-cares**, and only using exact methods where they are computationally justified.

A strong optimizer should therefore be built around this principle:

1. **Use exact minimization only on small windows or small-support functions.**
2. **Use heuristic cover minimization for SOP/POS-style problems.**
3. **Use network-level rewriting and resubstitution for large circuits.**
4. **Exploit observability and satisfiability-based don't-cares whenever possible.**
5. **Make the optimization objective explicit**: area, literal count, delay, LUT count, switching activity, or a weighted mix.
6. **Keep representations fluid**: truth tables for tiny cuts, cubes for covers, AIG/MIG/XAG-style graphs for network restructuring, BDDs for canonical reasoning and verification.

The practical lesson is simple: **two-level minimization is only one layer of circuit compression, not the whole story**.


## 1. What “circuit compression” really means

The phrase *circuit compression* is broader than *logic minimization*.

- **Logic minimization** usually means simplifying a Boolean function while preserving its behavior.
- **Circuit compression** is the more implementation-relevant goal: reducing the physical or structural cost of a circuit under one or more metrics.

Depending on the target, “smaller” may mean:

- fewer product terms,
- fewer literals,
- fewer gates,
- fewer AIG nodes,
- smaller mapped area,
- lower depth,
- lower switching activity,
- fewer LUTs after FPGA mapping,
- or better area-delay tradeoff.

This distinction matters because an expression that is minimal as a two-level SOP is **not necessarily** the best implementation after technology mapping. A representation with slightly more literals can map to fewer gates or fewer LUTs after rewriting and structural sharing.

### 1.1 Why the classic framing is incomplete

A survey centered on Karnaugh maps, Quine–McCluskey, Espresso, BDDs, and SAT is useful, but it still leaves out the most important practical point:

> Modern synthesis is driven as much by **representation and local restructuring** as by symbolic minimization.

For realistic circuits, the main optimization engine is often not “find the minimum SOP,” but rather:

- decompose logic into an efficient network,
- enumerate local cuts,
- rewrite subgraphs into better forms,
- rebalance logic levels,
- perform resubstitution using existing divisors,
- exploit don't-cares,
- remap the result to the target technology.

That is why systems such as **ABC** are built around scalable graph-based transformations on **And-Inverter Graphs (AIGs)** rather than around pure cover minimization alone.

---

## 2. The first major correction: separate two-level and multi-level optimization

The original document explains two-level minimization well enough, but a stronger document must clearly distinguish two different problem classes.

### 2.1 Two-level minimization

A Boolean function is implemented as a sum of products (SOP) or product of sums (POS). The optimization objective is typically:

- minimum number of cubes,
- then minimum number of literals,
- sometimes exact cover under don't-cares.

This is the natural domain of:

- Karnaugh maps,
- Quine–McCluskey,
- Petrick’s method,
- Espresso,
- SAT-based cover operators.

This matters for:

- PLA-style logic,
- local logic extraction,
- small support functions,
- certain preprocessing and exact benchmarks.

### 2.2 Multi-level optimization

A circuit is treated as a directed acyclic network of logic operators, not as one flat SOP. The optimizer is free to factor logic, share subexpressions, move inversions, change decomposition, and rewrite local cones.

This is the natural domain of:

- factored forms,
- AIG/XAG/MIG-style representations,
- cut-based rewriting,
- refactoring,
- balancing,
- resubstitution,
- don't-care based resynthesis,
- technology mapping.

This matters for:

- ASIC flows,
- FPGA flows,
- large arithmetic/control logic,
- industrial netlists.

### 2.3 Why the distinction is non-negotiable

A two-level minimum can be globally poor. For example, a function may have a compact factored form but an expensive SOP. Conversely, a locally excellent SOP can destroy opportunities for structural sharing across outputs.

So the improved decision rule is:

- **If the support is small and the objective is exact cover quality, use two-level methods.**
- **If the circuit is large, optimize the network, not just the expression.**

---

## 3. A stronger algorithm taxonomy

A better document should classify approaches by both **search strategy** and **representation**.

### 3.1 Exact cover-based methods

These methods aim for provable optimality, usually on two-level forms.

- Quine–McCluskey
- Petrick’s method
- exact SAT-based synthesis or covering on small windows
- exact truth-table based cut optimization

**Best use:** small support functions, local windows, verification of heuristics.

**Wrong use:** whole large circuits.

### 3.2 Heuristic cover minimizers

These methods work on cube covers and try to produce very good results fast.

- Espresso and its descendants
- SAT-accelerated cover operators
- sparse cover heuristics

**Best use:** sparse Boolean covers, PLA-style representations, local cover extraction.

### 3.3 Canonical symbolic methods

These represent logic canonically or semi-canonically for reasoning.

- ROBDDs / OBDDs
- ZDDs for sparse combinational set manipulation

**Best use:** verification, equivalence checking, some classes of structured functions, symbolic transformations.

**Important correction:** BDDs are not primarily a generic “minimizer”; they are mainly a **representation for reasoning and manipulation**. Their compression power depends heavily on variable order.

### 3.4 Graph/network-based optimization

These methods optimize the structure of a Boolean network directly.

- AIG rewriting
- balancing
- refactoring
- resubstitution
- rewriting with precomputed NPN classes
- cut-based exact replacement
- don't-care based resynthesis

**Best use:** large circuits, industrial flows, repeated optimization passes.

### 3.5 Solver-driven exact or semi-exact methods

These recast local optimization as SAT or related problems.

- SAT-based reduction
- SAT-based irredundancy tests
- SAT-based don't-care computation
- SAT-based exact synthesis for cuts/windows

**Best use:** local exactness inside a larger heuristic flow.

### 3.6 Metaheuristics and learned optimization

These include:

- genetic algorithms,
- simulated annealing,
- reinforcement learning / ML-guided flows,
- e-graph exploration with learned or technology-aware extraction.

**Best use:** exploration, flow tuning, nonstandard objectives, emerging research.

**Not a first-line replacement** for mature graph-based synthesis on standard objectives.

---

## 4. Exact two-level minimization: useful, but only in the right scope

### 4.1 Quine–McCluskey is still valuable

Quine–McCluskey remains important because it gives a deterministic exact framework:

1. generate all prime implicants,
2. select a minimum cover.

The method is still excellent when you need:

- a gold-standard exact answer for small functions,
- regression tests for heuristic engines,
- educational clarity,
- exact local optimization inside bounded windows.

### 4.2 The real bottleneck is not just prime generation

A stronger explanation should emphasize that the difficulty is **twofold**:

1. the number of prime implicants can explode,
2. the remaining cover problem is itself combinatorial.

This is the reason exact minimization fails at scale. The problem is not merely “many terms,” but a combination of:

- exponential candidate generation,
- NP-hard cover selection,
- large intermediate symbolic objects.

### 4.3 Practical correction: exactness belongs in windows

A serious optimizer should not expose Quine–McCluskey as a whole-circuit strategy. It should use exact methods only when:

- the local cut size is below a threshold,
- the truth table is small enough to fit efficiently in machine words,
- exact replacement is likely to improve cost significantly.

That leads directly to a better architecture:

- **global heuristic flow**,
- **local exact subproblems**.

### 4.4 Better exact alternatives than a pure Q–M implementation

For some local tasks, a modern implementation may prefer:

- exact SAT synthesis,
- exact NPN-class lookup tables for small cuts,
- precomputed optimal subcircuits,
- bounded exact replacement.

These often outperform a literal Quine–McCluskey implementation in practice while keeping exactness where it matters.

---

## 5. Espresso remains important, but the explanation should be sharper

### 5.1 What Espresso actually does well

Espresso succeeds because it does **not** try to enumerate all primes or solve the cover globally exactly. Instead, it repeatedly improves a valid cover through operators such as:

- **EXPAND**: make cubes larger without hitting the OFF-set,
- **IRREDUNDANT**: remove cubes not needed for coverage,
- **REDUCE**: shrink cubes strategically so later expansions can find better local minima.

The best intuitive explanation is this:

- **EXPAND** is greedy compression,
- **IRREDUNDANT** is cleanup,
- **REDUCE** is controlled damage that creates new search opportunities.

Without REDUCE, the algorithm gets trapped too early.

### 5.2 Stronger implementation advice

A serious document should emphasize that Espresso-style minimization benefits greatly from:

- packed bitset representations for cubes,
- fast set-containment tests,
- incremental cover bookkeeping,
- efficient sparse handling for ON/OFF/DC sets,
- careful cube ordering heuristics.

The quality of an Espresso-like engine depends heavily on these low-level choices.

### 5.3 What Espresso should not be oversold as

Espresso is not a universal solution to all logic compression tasks.

It is outstanding for **two-level covers**, but it is not the same thing as:

- full network restructuring,
- technology mapping,
- path balancing,
- cross-output global graph sharing.

A better document should say this explicitly so the reader does not mistake a cover minimizer for a full synthesis engine.

---

## 6. The missing centerpiece: graph-based rewriting for real circuit compression

This is the largest conceptual improvement the document needs.

### 6.1 Why graph representations matter

For large circuits, the optimizer should work over a compact structural graph such as an **AIG**:

- each node is a 2-input AND,
- inversion is represented on edges or as complemented attributes,
- structure sharing is explicit,
- local replacement becomes cheap.

This representation is powerful because many transformations become simple graph edits rather than global algebraic rewrites.

### 6.2 The standard modern flow

A good large-scale compression flow often looks like this:

1. **strash**: structurally hash the network to merge identical nodes,
2. **balance**: reduce logic depth without increasing area too much,
3. **rewrite**: replace local cuts with cheaper equivalent structures,
4. **refactor**: extract better algebraic decomposition,
5. **resubstitute**: reuse existing divisors/signals instead of building new logic,
6. **use don't-cares** to legalize more aggressive changes,
7. **map** to the target technology,
8. optionally iterate because mapping changes what is worth optimizing.

That is much closer to how serious circuit compression is done than a flat “choose one minimization algorithm” perspective.

### 6.3 DAG-aware rewriting

DAG-aware rewriting is especially important because a local replacement should not be judged only by the replacement subgraph itself, but by its effect on the **shared network**.

A replacement that seems neutral or slightly worse in isolation may become better because it:

- enables node sharing elsewhere,
- reduces fanout duplication,
- preserves depth,
- improves mapping opportunities.

This is why graph-based synthesis must be **DAG-aware**, not only tree-aware.

### 6.4 Resubstitution: one of the most powerful missing ideas

Resubstitution is underrepresented in many older surveys but is central in practice.

The idea is to express a node using already-existing signals in the network, rather than building new logic from scratch. This can significantly reduce area because the optimizer reuses logic that has already been paid for.

A good document should explain that resubstitution is often stronger than naive local minimization because it exploits **global availability** of internal divisors.

### 6.5 Why this changes the implementation recommendation

If the goal is serious compression of realistic circuits, the best foundation is usually:

- a graph network representation,
- cut enumeration,
- local truth-table computation,
- rewriting/resubstitution engines,
- optional exact replacement on tiny cuts.

This is the biggest strategic improvement over the original document.

---

## 7. Don’t-cares are not a side topic; they are a compression multiplier

A major weakness in many explanations is that don't-cares are treated as a small convenience. In reality they are one of the strongest sources of compression.

### 7.1 Types of don't-cares

A better document should distinguish at least:

- **external don't-cares (EXDCs)**: unspecified input combinations,
- **satisfiability don't-cares (SDCs)**: combinations impossible because of internal logic constraints,
- **observability don't-cares (ODCs)**: internal node changes that do not affect primary outputs under relevant conditions,
- **complete don't-cares (CDCs)** as a combined practical notion depending on context.

### 7.2 Why they matter

Don't-cares enlarge the legal optimization space. They allow:

- larger cube expansion,
- more aggressive rewriting,
- cheaper resubstitution,
- alternative decompositions that would otherwise be illegal.

### 7.3 Modern point that should be emphasized

Scalable synthesis does not merely *use* don't-cares after the fact. It often computes them locally around cuts or windows because full global don't-care computation is too expensive.

This yields a practical compromise:

- approximate or localized don't-cares,
- but enough to unlock major optimizations.

That is a much stronger implementation message than simply saying “don't-cares help simplification.”

---

## 8. SAT is not just a solver add-on; it is a local exactness engine

### 8.1 Better interpretation of SAT-based methods

SAT-based minimization should not be presented only as “Espresso but faster.” A stronger framing is:

> SAT provides a way to inject **exact reasoning** into selected hard subproblems without turning the entire synthesis flow into a global exact algorithm.

That is the real strength.

### 8.2 Where SAT works best

SAT is especially effective for:

- local equivalence checks,
- irredundancy checks,
- witness generation,
- exact cut replacement,
- don't-care computation,
- exact synthesis of small subfunctions,
- proving legality of aggressive transformations.

### 8.3 What a strong implementation does

A strong implementation uses SAT selectively:

- heuristic graph traversal finds candidates,
- local cuts are enumerated,
- a SAT engine proves equivalence or legality,
- the replacement is accepted only if the cost improves.

This hybrid model is much better than trying to solve the whole circuit globally in one SAT formulation.

### 8.4 Important engineering note

When SAT is used repeatedly inside a synthesis loop, performance depends on:

- incremental solving,
- clause reuse where possible,
- compact CNF generation,
- good candidate filtering before solver calls,
- simulation signatures to reject obvious losers before invoking SAT.

This point substantially improves the algorithmic realism of the document.

---

## 9. BDDs need a more precise role

### 9.1 What BDDs are excellent at

BDDs are outstanding when you need:

- canonical comparison under a fixed order,
- symbolic manipulation,
- equivalence checking,
- structured function representation,
- certain forms of don't-care or image/preimage style reasoning.

### 9.2 What should be corrected in the narrative

A stronger document should clearly state:

- BDDs are not the universal answer to compression.
- Their size may collapse beautifully or blow up exponentially depending on variable ordering.
- They are often best used as **auxiliary reasoning tools**, not as the sole optimization substrate.

### 9.3 When BDDs are still strategically useful

They remain very useful for:

- local windows with manageable support,
- exact symbolic manipulation,
- verification-oriented flows,
- hybrid methods where graph optimization uses symbolic checks only on selected cones.

---

## 10. Exact synthesis on small cuts: a major algorithmic upgrade

A serious improvement over the original document is to add a section on **exact synthesis for small cuts**.

### 10.1 The idea

Instead of globally minimizing a large circuit, take a small cut of the network:

- typically 4 to 8 inputs in many practical flows,
- compute the cut function as a truth table,
- derive an optimal or near-optimal replacement under the chosen cost model,
- splice it back into the network.

### 10.2 Why this is powerful

This gives you the benefits of exact methods without global explosion. It also lets the optimizer target more realistic costs:

- node count,
- depth,
- target-library cost,
- LUT count,
- fanout-sensitive cost.

### 10.3 Practical sources of exact local replacements

A production-quality engine can use:

- NPN canonicalization,
- precomputed optimal implementations for common classes,
- SAT-based exact synthesis when cache miss occurs,
- memoized replacement libraries.

This section is crucial because it bridges the gap between “exact theory” and “scalable synthesis practice.”

---

## 11. Technology-aware optimization must be moved earlier in the story

The original document is mostly technology-independent. That is acceptable for a theoretical survey, but not for a document about practical circuit compression.

### 11.1 Why technology matters

A representation that looks good before mapping may be bad after mapping.

Examples:

- an ASIC flow may care about gate area and critical path,
- an FPGA flow may care about k-LUT packing and cut structure,
- XOR-rich logic may benefit from XAG-style handling instead of plain AIG-only optimization,
- arithmetic blocks may respond differently to balancing and decomposition.

### 11.2 Better recommendation

The optimizer should expose a **cost model abstraction**. Every transformation should be evaluated against one or more selectable objectives:

- area-only,
- delay-only,
- area-delay product,
- LUT count,
- switching activity,
- weighted composite score.

### 11.3 Practical implication

This means the document should recommend not a single optimizer, but a **framework** where:

- local function extraction,
- legality checking,
- replacement generation,
- and cost evaluation

are separate modules.

That design scales much better than hard-wiring one algorithmic worldview into the whole tool.

---

## 12. A better architecture for an actual implementation

Below is a more serious design for a logic/circuit compression engine.

### 12.1 Core layers

#### Layer A: input normalization

- parse Verilog/BLIF/AIGER/PLA,
- hash constants and complemented edges,
- structural hashing (strashing),
- normalize fanin ordering for canonical node creation,
- maintain equivalence and reference counts.

#### Layer B: multiple internal representations

Use the right representation for the right job.

- **truth tables** for tiny cuts,
- **cube covers** for two-level operations,
- **AIG/XAG/MIG-style graphs** for network optimization,
- **BDD/ZDD** only where symbolic reasoning is advantageous.

#### Layer C: fast analysis infrastructure

- topological order,
- fanout counts,
- level/depth computation,
- random simulation signatures,
- cut enumeration,
- NPN canonicalization,
- local observability windows.

#### Layer D: transformation engines

- balancing,
- rewriting,
- refactoring,
- resubstitution,
- don't-care based resynthesis,
- cover extraction and Espresso-like minimization,
- SAT-based local exact replacement.

#### Layer E: cost and legality layer

- equivalence check,
- incremental SAT confirmation on ambiguous candidates,
- technology-aware scoring,
- rollback for failed replacements.

### 12.2 Recommended iterative flow

```text
normalize -> strash -> balance
repeat
    enumerate cuts/windows
    try rewrite/refactor/resubstitute
    compute local don't-cares where worthwhile
    try exact replacement on high-value small cuts
    clean up redundancies
    rebalance if depth drifted badly
until no meaningful gain
map to target technology
post-map local improvement if supported
```

### 12.3 Why this is stronger

This architecture makes room for:

- exact algorithms,
- heuristic cover minimization,
- graph-based scalability,
- technology-awareness,
- verification.

It reflects modern synthesis much more faithfully than a document built around only two-level minimization.

---

## 13. Better explanation of the trade space

The original “trilemma” of optimality, speed, and scalability is useful, but it should be upgraded.

### 13.1 A more realistic optimization tetrahedron

A practical optimizer trades off at least four axes:

1. **local optimality**,
2. **global scalability**,
3. **representation suitability**,
4. **technology relevance**.

A method can be excellent on one representation and poor on another. That matters as much as raw asymptotic complexity.

### 13.2 Examples

- Quine–McCluskey: strong local optimality, weak scalability.
- Espresso: strong two-level practicality, weak as a complete network optimizer.
- BDDs: strong symbolic reasoning, highly order-sensitive.
- AIG rewriting: excellent scalability and good practical QoR, but not globally exact.
- SAT local exactness: high-quality local decisions, depends on good candidate filtering.
- E-graphs: broader search space, but extraction cost and scalability must be controlled.

---

## 14. E-graphs and newer approaches: promising, but not yet the baseline

A serious revision should mention newer directions without overselling them.

### 14.1 Why e-graphs are interesting

E-graphs let the optimizer maintain many equivalent rewrites simultaneously instead of greedily committing to one rewrite at a time. This can explore a wider design space than classic local rewriting.

### 14.2 Why caution is needed

The extraction problem becomes critical:

- too many equivalent forms can explode search space,
- cost extraction must be technology-aware,
- unrestricted equality saturation can become too expensive.

### 14.3 Best framing

E-graphs should be presented as an **emerging complement** to graph-based rewriting, not as a universal replacement for AIG-based flows.

---

## 15. Stronger comparative table

| Approach | Main representation | Exact? | Scales to large circuits? | Best role | Main weakness |
|---|---|---:|---:|---|---|
| Karnaugh maps | grid / truth table intuition | Yes for tiny cases | No | teaching, manual sanity checks | not programmable at scale |
| Quine–McCluskey + Petrick | cubes / implicants | Yes | No | exact small-support minimization | exponential blow-up |
| Espresso | cube covers | No, but high quality | Moderate to good | two-level heuristic minimization | not a full network optimizer |
| ROBDD/OBDD | canonical decision graph | Canonical for fixed order | Highly variable | equivalence, symbolic reasoning | variable-order explosion |
| SAT-based local minimization | CNF + local functions | Local exactness | Good when localized | exact subroutines inside heuristics | expensive if overused |
| AIG rewriting / balancing | graph network | No | Yes | industrial-scale compression | local view can miss broader alternatives |
| Resubstitution | graph + divisors | No | Yes | area reduction through reuse | candidate selection is nontrivial |
| Don’t-care based resynthesis | graph + local constraints | No / semi-exact | Good if localized | unlock aggressive legal rewrites | DC computation can dominate runtime |
| Exact cut synthesis | truth tables on cuts | Yes locally | Yes if bounded | best-in-class local replacement | bounded support only |
| E-graphs | equivalence graph | No in general | Emerging | broader rewrite exploration | extraction and memory cost |

---

## 16. Recommendations rewritten more seriously

### 16.1 If the goal is an educational exact minimizer

Implement:

- Quine–McCluskey,
- Petrick’s method,
- don't-care support,
- exact regression suite.

But label it clearly as a **small-function engine**.

### 16.2 If the goal is a practical two-level minimizer

Implement:

- Espresso-style cube operations,
- bit-packed cubes,
- incremental irredundancy bookkeeping,
- optional SAT-assisted irredundancy and reduction.

### 16.3 If the goal is serious circuit compression for realistic designs

Build around:

- AIG or related network representation,
- structural hashing,
- cut enumeration,
- balancing,
- rewriting,
- resubstitution,
- localized don't-care based optimization,
- exact replacement only on small high-value windows,
- technology-aware cost evaluation.

This is the strongest recommendation for a modern implementation.

### 16.4 If the goal includes verification synergy

Add:

- BDD support for selected symbolic tasks,
- SAT-based equivalence checking,
- FRAIG-style or structurally hashed equivalence handling where appropriate,
- proof-producing legality checks if the environment requires high assurance.

### 16.5 If the goal is research exploration

Then and only then promote:

- e-graphs,
- ML-guided transformation ordering,
- stochastic/metaheuristic search,
- learned cost models.

These are valuable, but they should be layered on top of a strong deterministic baseline, not used as a substitute for one.

---

## 17. Concrete algorithmic improvements the original document should adopt

To make the document substantially stronger, the following changes should be made explicitly.

### 17.1 Improvement 1: stop treating “logic minimization” as mostly a two-level problem

Two-level minimization should become one section, not the center of gravity of the whole report.

### 17.2 Improvement 2: add network-level optimization as the main practical engine

AIG rewriting, balancing, refactoring, and resubstitution should be elevated to first-class topics.

### 17.3 Improvement 3: explain don't-cares as a central optimization resource

They should be presented as one of the main reasons modern optimizers beat purely algebraic simplifiers.

### 17.4 Improvement 4: present SAT as a selective exact oracle

SAT is strongest when injected locally, not when used as a monolithic whole-circuit optimizer.

### 17.5 Improvement 5: add exact synthesis on bounded cuts

This is one of the best bridges between exact theory and scalable practice.

### 17.6 Improvement 6: make technology awareness explicit

Optimization objectives should be tied to ASIC/FPGA/library realities.

### 17.7 Improvement 7: improve the explanation of representation choice

The document should repeatedly state that the “best algorithm” depends on the representation.

### 17.8 Improvement 8: include implementation-level engineering choices

For example:

- bitset encoding of cubes,
- NPN canonicalization caches,
- cut memoization,
- random simulation prefilters,
- incremental SAT usage,
- node reference counting and rollback.

These are exactly the kinds of details that separate a readable survey from a useful design document.

---

## 18. Final recommendation

If the target is a real compression tool rather than an academic survey, the most serious conclusion is this:

> Build a **hybrid synthesis framework**.
>
> - Use **Espresso-like cover minimization** where the problem is naturally two-level.
> - Use **AIG-based graph optimization** as the primary large-scale engine.
> - Use **SAT and exact synthesis** only for small local windows where exactness is affordable.
> - Use **don't-cares** aggressively but locally.
> - Keep the cost model **technology-aware** from the start.

That recommendation is much stronger, more modern, and much closer to successful synthesis practice than a document centered mostly on Quine–McCluskey, Espresso, BDDs, and metaheuristics alone.

---

## Selected references

The list below is intentionally narrower and more authoritative than a broad web bibliography. It favors original papers, major technical reports, and tool-defining references.

1. R. E. Bryant, “Graph-Based Algorithms for Boolean Function Manipulation,” *IEEE Transactions on Computers*, 1986.
2. R. E. Bryant, “Symbolic Boolean Manipulation with Ordered Binary Decision Diagrams,” *ACM Computing Surveys*, 1992.
3. R. K. Brayton, G. D. Hachtel, C. T. McMullen, A. Sangiovanni-Vincentelli, *Logic Synthesis for VLSI Design*, Kluwer, 1984/1989.
4. R. Rudell, “Multiple-Valued Logic Minimization for PLA Synthesis,” PhD thesis / Berkeley technical report, 1986.
5. S. Sapra, M. Theobald, E. Clarke, “SAT-Based Algorithms for Logic Minimization,” ICCD 2003.
6. R. Brayton, A. Mishchenko, “ABC: An Academic Industrial-Strength Verification Tool,” CAV 2010.
7. A. Mishchenko, S. Chatterjee, R. Brayton, “DAG-Aware AIG Rewriting: A Fresh Look at Combinational Logic Synthesis,” DAC 2006.
8. A. Mishchenko, R. Brayton, J.-H. Jiang, S. Jang, “Scalable Don’t-Care-Based Logic Optimization and Resynthesis,” *ACM TRETS*, 2011.
9. A. Mishchenko, R. Brayton, “Scalable Logic Synthesis Using a Simple Circuit Structure,” IWLS 2006.
10. M. Soeken et al., “Practical Exact Synthesis,” DATE 2018.
11. S.-Y. Lee et al., “Simulation-Guided Boolean Resubstitution,” 2020.
12. A. Costamagna et al., “An Enhanced Resubstitution Algorithm for Area-Oriented Logic Optimization,” ISCAS 2024.
13. C. Chen et al., “E-Syn: E-Graph Rewriting with Technology-Aware Cost Functions for Logic Synthesis,” DAC 2024.

