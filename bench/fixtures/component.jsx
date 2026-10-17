import { useEffect, useMemo, useState } from "react";

const currency = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });

class Cart {
  #items = new Map();

  add(product, quantity = 1) {
    const current = this.#items.get(product.id)?.quantity ?? 0;
    this.#items.set(product.id, { ...product, quantity: current + quantity });
  }

  remove(id) {
    this.#items.delete(id);
  }

  get total() {
    return [...this.#items.values()].reduce((sum, { price, quantity }) => sum + price * quantity, 0);
  }

  get lines() {
    return [...this.#items.values()];
  }
}

async function loadProducts(category, { signal } = {}) {
  const response = await fetch(`/api/products?category=${encodeURIComponent(category)}`, { signal });
  if (!response.ok) throw new Error(`products: ${response.status}`);
  const { products = [] } = await response.json();
  return products.map(({ id, name, price, tags = [] }) => ({ id, name, price, tags }));
}

function ProductRow({ product, onAdd }) {
  const label = product.tags.length ? ` (${product.tags.join(", ")})` : "";
  return (
    <tr>
      <td>{product.name}{label}</td>
      <td className="price">{currency.format(product.price)}</td>
      <td>
        <button type="button" onClick={() => onAdd(product)} disabled={product.price <= 0}>
          Add
        </button>
      </td>
    </tr>
  );
}

export default function ProductList({ category = "all", initialProducts = [] }) {
  const [products, setProducts] = useState(initialProducts);
  const [query, setQuery] = useState("");
  const [error, setError] = useState(null);
  const cart = useMemo(() => new Cart(), []);
  const [, setVersion] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    loadProducts(category, controller)
      .then(setProducts)
      .catch((e) => e.name !== "AbortError" && setError(e.message));
    return () => controller.abort();
  }, [category]);

  const visible = useMemo(
    () => products.filter(({ name }) => name.toLowerCase().includes(query.trim().toLowerCase())),
    [products, query],
  );

  const add = (product) => {
    cart.add(product);
    setVersion((v) => v + 1);
  };

  if (error) return <p role="alert">Could not load products: {error}</p>;

  return (
    <section>
      <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Filter" />
      <table>
        <tbody>
          {visible.map((product) => <ProductRow key={product.id} product={product} onAdd={add} />)}
        </tbody>
      </table>
      <footer>
        {cart.lines.length} items, {currency.format(cart.total)}
      </footer>
    </section>
  );
}
