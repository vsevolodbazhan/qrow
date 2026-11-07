CREATE TABLE customers (
    customer_id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL,
    email text NOT NULL UNIQUE,
    city text NOT NULL,
    joined_on date NOT NULL
);

CREATE TABLE products (
    product_id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL,
    category text NOT NULL,
    unit_price numeric(10, 2) NOT NULL CHECK (unit_price >= 0),
    in_stock boolean NOT NULL DEFAULT true
);

CREATE TABLE orders (
    order_id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    customer_id integer NOT NULL REFERENCES customers,
    ordered_at timestamptz NOT NULL,
    status text NOT NULL CHECK (status IN ('pending', 'paid', 'shipped', 'cancelled')),
    total numeric(12, 2) NOT NULL CHECK (total >= 0),
    notes text
);

CREATE TABLE order_items (
    order_id integer NOT NULL REFERENCES orders ON DELETE CASCADE,
    product_id integer NOT NULL REFERENCES products,
    quantity integer NOT NULL CHECK (quantity > 0),
    unit_price numeric(10, 2) NOT NULL,
    PRIMARY KEY (order_id, product_id)
);

INSERT INTO customers (name, email, city, joined_on) VALUES
    ('Ada Lovelace', 'ada@example.test', 'London', '2023-01-15'),
    ('Grace Hopper', 'grace@example.test', 'Arlington', '2023-03-02'),
    ('Katherine Johnson', 'katherine@example.test', 'Hampton', '2024-06-18'),
    ('Alan Turing', 'alan@example.test', 'Manchester', '2024-09-09');

INSERT INTO products (name, category, unit_price, in_stock) VALUES
    ('Mechanical keyboard', 'Accessories', 89.95, true),
    ('USB-C hub', 'Accessories', 42.50, true),
    ('27-inch monitor', 'Displays', 329.00, true),
    ('Laptop stand', 'Accessories', 54.25, false),
    ('Webcam', 'Cameras', 74.99, true);

INSERT INTO orders (customer_id, ordered_at, status, total, notes) VALUES
    (1, '2025-01-12 10:20:00+00', 'shipped', 132.45, 'Deliver to reception'),
    (2, '2025-02-03 14:05:00+00', 'paid', 329.00, NULL),
    (1, '2025-02-18 09:30:00+00', 'pending', 74.99, 'Gift wrap requested'),
    (3, '2025-03-01 16:45:00+00', 'cancelled', 54.25, NULL),
    (4, '2025-03-08 11:15:00+00', 'shipped', 89.95, 'Leave at front desk');

INSERT INTO order_items (order_id, product_id, quantity, unit_price) VALUES
    (1, 1, 1, 89.95), (1, 2, 1, 42.50),
    (2, 3, 1, 329.00),
    (3, 5, 1, 74.99),
    (4, 4, 1, 54.25),
    (5, 1, 1, 89.95);

CREATE VIEW order_summary AS
SELECT o.order_id, c.name AS customer, o.ordered_at, o.status, o.total,
       count(i.product_id) AS item_count
FROM orders AS o
JOIN customers AS c USING (customer_id)
LEFT JOIN order_items AS i USING (order_id)
GROUP BY o.order_id, c.name;
